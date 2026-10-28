package handler

import (
	"net/http"
	"time"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"
)

// registerOps 注册运维视图。
//
// 与仪表盘的分工：
//   · 仪表盘回答"用了多少、好不好用"（面向使用者/管理者）；
//   · 运维回答"现在能不能扛住、哪里在漏"（面向排障）。
//
// 所以这里只放**运行态**：并发水位、网关自身计数、账号健康分布、告警。
// 不放 token、成本、模型排行 —— 那些是仪表盘的事，重复展示只会让人分不清该看哪。
func registerOps(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /ops/overview", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		// 窗口与粒度可从查询参数覆盖，默认 1 小时 / 5 分钟一桶。
		// 钳制在 store 层做（15 分钟 ~ 24 小时）。
		windowMinutes := httpx.IntQuery(r, "windowMinutes", 60)
		bucketMinutes := httpx.IntQuery(r, "bucketMinutes", 5)

		/*
			性能指标先查：它是**纯数据库聚合**，即使网关挂了也能算出来
			（历史请求已经落库）。所以它不该跟网关状态绑在一起 ——
			"网关刚重启"时正是最需要看趋势的时候。

			查询失败也不阻断页面：降级成 nil，前端显示"指标不可用"，
			而不是整页白屏。
		*/
		var metrics any
		if value, err := ctx.Store.GetOpsMetrics(r.Context(), windowMinutes, bucketMinutes); err == nil {
			metrics = value
		} else {
			logx.Infof("运维指标不可用: %v", err)
		}

		status, err := ctx.Gateway.GetStatus(r.Context())
		if err != nil {
			// 网关不可用**本身**就是最重要的运维信息，不能报 500 让页面白屏。
			// 返回 200 + `gatewayOnline: false`，页面据此展示"网关离线"与可能的原因。
			httpx.OK(w, map[string]any{
				"gatewayOnline": false,
				"gatewayError":  err.Error(),
				"checkedAt":     time.Now().Format(time.RFC3339),
				"metrics":       metrics,
			})
			return
		}

		// 账号健康分布：按状态聚合，用于"有多少号现在用不了"。
		// 与网关的 `accounts[]` 一致，不再单独查库 —— 运行态只有一个来源。
		var ready, cooling, noCredential, disabled int
		inflight := 0
		capacity := 0
		for _, account := range status.Accounts {
			inflight += account.Inflight
			capacity += account.MaxInflight

			switch {
			case !account.Enabled:
				disabled++
			case account.RequiresCredential && !account.HasCredential:
				noCredential++
			case len(account.BlockedReasons) > 0:
				cooling++
			case account.Ready:
				ready++
			}
		}

		// 全局并发水位：`available` 是网关那边还剩多少个名额。
		globalUsed := status.Concurrency.Global - status.Concurrency.Available
		if globalUsed < 0 {
			globalUsed = 0
		}

		httpx.OK(w, map[string]any{
			"gatewayOnline": true,
			"checkedAt":     time.Now().Format(time.RFC3339),

			"gateway": map[string]any{
				"running":  status.Running,
				"addr":     status.Addr,
				"version":  status.Version,
				"uptimeMs": status.UptimeMs,
				"auth":     status.Auth,
			},

			"concurrency": map[string]any{
				"global":    status.Concurrency.Global,
				"available": status.Concurrency.Available,
				"used":      globalUsed,
				// 账号级在飞 / 容量之和：能看出"是全局闸门卡住，还是账号自己满了"。
				"accountInflight": inflight,
				"accountCapacity": capacity,
			},

			// 网关进程内计数（自启动累计，不是数据库聚合）。
			"counters": map[string]any{
				"requests": status.Stats.Requests,
				"failures": status.Stats.Failures,
				"retries":  status.Stats.Retries,
			},

			"matrix": map[string]any{
				"routes":     status.Matrix.Routes,
				"live":       status.Matrix.Live,
				"builtin":    status.Matrix.Builtin,
				"ageMs":      status.Matrix.AgeMs,
				"ttlSeconds": status.Matrix.TTLSeconds,
			},

			"accounts": map[string]any{
				"total":        len(status.Accounts),
				"ready":        ready,
				"cooling":      cooling,
				"noCredential": noCredential,
				"disabled":     disabled,
			},

			// 性能指标（TTFT / OTPS / 延迟分位 / 趋势）。
			// 这一块才是运维真正要看的：上面那些回答"进程活着吗"，
			// 这一块回答"跑得好不好"。
			"metrics": metrics,

			// 网关自己报的异常（加载失败的账号、矩阵构建问题等）。
			"warnings": status.Warnings,
		})
	})
}
