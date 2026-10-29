package handler

import (
	"net/http"
	"strconv"
	"time"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerStats 注册仪表盘与渠道健康度路由。
//
// 为什么不复用 `/usage/summary`：那个接口是"给我一段区间、按某个维度分组"，
// 服务于「统计报表」页的自由查询；仪表盘要的是**一次拿全**的固定快照
// （今日 / 累计 / 趋势 / 多维度拆分）。两者语义不同，合并会让两边都别扭：
// 报表页被迫多取用不上的字段，仪表盘被迫发 8 个请求拼一个快照。
func registerStats(mux *http.ServeMux, ctx *svc.ServiceContext) {
	// 仪表盘总览。
	mux.HandleFunc("GET /dashboard", func(w http.ResponseWriter, r *http.Request) {
		to := parseTimeQuery(r, "to", time.Now())
		from := parseTimeQuery(r, "from", to.Add(-24*time.Hour))
		bucket := "hour"
		if r.URL.Query().Get("bucket") == "day" {
			bucket = "day"
		}

		overview, err := ctx.Store.GetDashboardOverview(r.Context(), from, to, bucket)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, overview)
	})

	// 渠道健康度（「渠道状态」页的卡片数据）。
	//
	// `days` 控制统计窗口；可用账号数来自网关，网关不可用时该字段为 null
	// 而不是 0 —— 界面据此显示"—"而不是"0/2 可用"，后者会让人以为号全掉了。
	mux.HandleFunc("GET /channels/health", func(w http.ResponseWriter, r *http.Request) {
		days := httpx.IntQuery(r, "days", 7)
		if days < 1 {
			days = 1
		}
		if days > 90 {
			days = 90
		}

		from := time.Now().Add(-time.Duration(days) * 24 * time.Hour)
		list, err := ctx.Store.ListChannelHealth(r.Context(), from)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 补上网关的运行态。网关不可用时静默跳过：卡片仍要能显示历史统计，
		// 只是"可用账号"一栏变成未知。
		if status, err := ctx.Gateway.GetStatus(r.Context()); err == nil && status != nil {
			attachChannelRuntime(list, status)
		}

		httpx.OK(w, list)
	})
}

// attachChannelRuntime 按账号的真实分组汇总可用数。
// status.Channels 来自平台适配器注册表（如 workbuddy），不能用它匹配
// 用户分组（如 wb-cn / wb-intl），也不能把同平台的账号数复制到各分组。
// 网关运行时，未加载账号的分组可用数为 0；只有离线时才保留未知值。
func attachChannelRuntime(list []store.ChannelHealth, status *gateway.Status) {
	for position := range list {
		list[position].ReadyAccounts = nil
	}
	if status == nil || !status.Running {
		return
	}

	ready := make(map[string]int)
	for _, account := range status.Accounts {
		if account.Enabled && account.Ready {
			ready[account.ChannelID]++
		}
	}
	for position := range list {
		count := ready[list[position].ID]
		list[position].ReadyAccounts = &count
	}
}

// parseTimeQuery 读 ISO 8601 时间参数；缺省或非法时用 fallback。
//
// 非法值不报 400：时间参数是展示范围，写错了退回默认比整个页面报错更有用。
func parseTimeQuery(r *http.Request, name string, fallback time.Time) time.Time {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return fallback
	}

	if parsed, err := time.Parse(time.RFC3339, raw); err == nil {
		return parsed
	}
	// 也接受 epoch 毫秒，前端算区间时两种都可能用上。
	if millis, err := strconv.ParseInt(raw, 10, 64); err == nil {
		return time.UnixMilli(millis)
	}
	return fallback
}
