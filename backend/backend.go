package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"relayai/backend/internal/config"
	"relayai/backend/internal/database"
	"relayai/backend/internal/handler"
	"relayai/backend/internal/svc"

	"github.com/zeromicro/go-zero/core/conf"
	"github.com/zeromicro/go-zero/core/logx"
)

var configFile = flag.String("f", "etc/backend-api.yaml", "the config file")

func main() {
	flag.Parse()

	var c config.Config
	conf.MustLoad(*configFile, &c)

	// 环境变量优先于配置文件：生产环境不该把连接串写进仓库。
	if dsn := os.Getenv("DATABASE_URL"); dsn != "" {
		c.Database.DataSource = dsn
	}
	if token := os.Getenv("GATEWAY_INTERNAL_TOKEN"); token != "" {
		c.Gateway.InternalToken = token
	}
	// 网关端口被占用时会自动回退（参考实现的行为），所以地址必须可覆盖，
	// 否则管理面会一直打那个"本该是网关、实际是别人"的端口。
	if baseURL := os.Getenv("GATEWAY_BASE_URL"); baseURL != "" {
		c.Gateway.BaseURL = baseURL
	}
	if publicBaseURL := os.Getenv("GATEWAY_PUBLIC_BASE_URL"); publicBaseURL != "" {
		c.Gateway.PublicBaseURL = publicBaseURL
	}

	ctx := context.Background()

	db, err := database.Connect(ctx, c.Database.DataSource, database.Options{
		MaxOpenConns: c.Database.MaxConns,
		MaxIdleConns: c.Database.MinConns,
		MaxLifetime:  time.Hour,
		MaxIdleTime:  15 * time.Minute,
	})
	if err != nil {
		logx.Must(fmt.Errorf("数据库初始化失败: %w", err))
	}
	// 关闭走底层 sql.DB：GORM 本身没有 Close 方法。
	if sqlDB, closeErr := db.DB(); closeErr == nil {
		defer func() {
			if err := sqlDB.Close(); err != nil {
				logx.Errorf("关闭数据库连接失败: %v", err)
			}
		}()
	}

	if c.Database.AutoMigrate {
		if err := database.Migrate(ctx, db); err != nil {
			logx.Must(fmt.Errorf("数据库迁移失败: %w", err))
		}
	}

	svcCtx := svc.NewServiceContext(c, db)

	// 用标准库 http.Server 而不是 rest.Server：
	// 本 API 需要 PATCH 与 `{id}` 路径参数，go-zero 的路由器两者都不支持
	// （见 internal/handler/routes.go 顶部说明）。
	// go-zero 仍负责配置加载与日志。
	server := &http.Server{
		Addr:              fmt.Sprintf("%s:%d", c.Host, c.Port),
		Handler:           handler.RegisterHandlers(svcCtx),
		ReadHeaderTimeout: 10 * time.Second,
		// 不设 WriteTimeout：流式代理与长任务需要长连接。
		IdleTimeout: 120 * time.Second,
	}

	go func() {
		fmt.Printf("Starting server at %s:%d...\n", c.Host, c.Port)
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			logx.Must(fmt.Errorf("HTTP 服务异常退出: %w", err))
		}
	}()

	/*
		模型目录同步。

		模型清单只存在于网关的内存矩阵里（从各渠道适配器实时拉取），
		从不落库；而管理面的模型页与公开广场读的是 `models` 表。
		不同步的话那张表永远是空的，页面也就永远是空的。

		**启动后延迟 30 秒**：网关启动时要先建矩阵（拉各渠道的模型目录），
		刚起来的几秒内矩阵可能还是空的。立刻同步会拿到空清单，
		然后跳过清理 —— 无害但也无用，等于白跑一次。
		之后每小时同步一次，覆盖"上游新增/下线模型"。
	*/
	go func() {
		// 启动后先推一次价格：网关的 PriceBook 在内存里，进程刚起来是空的。
		// 不推的话，重启后到第一次同步之间（最长 1 小时）的请求全部记 0 成本。
		// 这里等 5 秒让网关先起来，比等 30 秒更早生效。
		time.Sleep(5 * time.Second)
		handler.PushPrices(context.Background(), svcCtx)

		timer := time.NewTimer(30 * time.Second)
		defer timer.Stop()

		for {
			<-timer.C
			if _, err := handler.SyncCatalog(context.Background(), svcCtx); err != nil {
				logx.Infof("模型目录同步失败（下一轮重试）: %v", err)
			}
			// 同步后必须**再推一次价格**：
			// 新同步进来的模型在网关侧没有价格，成本会算成 0，
			// 而那些行的价格快照也是空的 —— 事后看"为什么这条没记价"，查不出来。
			handler.PushPrices(context.Background(), svcCtx)
			timer.Reset(time.Hour)
		}
	}()

	/*
		用量结算。

		网关只负责算钱和记用量，**从不碰用户余额**（它的存储层连 `users` 表都不读）。
		结算在管理面做：把还没扣过钱的用量按 Key 归到用户，扣余额并写流水。

		为什么必须定时跑：用量一直在产生，靠"管理员想起来点一下"意味着
		余额长期是旧值 —— 用户看到的是"用了很多但余额没少"。

		间隔 1 分钟：结算本身很轻（只处理未结算的行，靠部分索引），
		而更长的间隔会让余额显示明显滞后。
	*/
	go func() {
		// 启动后先等 20 秒：让网关先把服务跑起来、迁移先执行完。
		timer := time.NewTimer(20 * time.Second)
		defer timer.Stop()

		for {
			<-timer.C
			handler.RunSettlement(svcCtx)
			timer.Reset(time.Minute)
		}
	}()

	/*
		定时探活。

		设置页里一直有「定时探活」开关，但**从来没有代码去执行它** ——
		开关是开的，用户以为有人盯着，其实没有；凭据过期要等真实请求失败才发现。

		这里按设置的间隔对每个启用账号发一句 hello。
		间隔每轮重新读，用户在设置页改完下一轮就生效。
	*/
	probeCtx, stopProbe := context.WithCancel(context.Background())
	defer stopProbe()
	go handler.RunProbeScheduler(probeCtx, svcCtx)

	// 优雅退出：收到信号后给在飞请求 10 秒收尾。
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
	<-stop

	fmt.Println("正在停止服务...")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	if err := server.Shutdown(shutdownCtx); err != nil {
		logx.Errorf("停止服务失败: %v", err)
	}
}
