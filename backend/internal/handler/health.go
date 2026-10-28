package handler

import (
	"net/http"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"
)

// registerHealth 注册健康检查与公开配置。
func registerHealth(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /health", func(w http.ResponseWriter, r *http.Request) {
		database := "down"
		if ctx.DB != nil {
			if err := ctx.DB.Ping(r.Context()); err == nil {
				database = "up"
			}
		}

		httpx.OK(w, map[string]any{
			"service":  "relayai-backend",
			"status":   "ok",
			"version":  serviceVersion,
			"database": database,
			"uptimeMs": ctx.UptimeMs(),
		})
	})

	// 公开配置：前端拼客户端接入信息（base URL / curl / CC Switch 导入）时需要。
	//
	// 为什么不能直接用 `window.location.origin`：
	// 管理台跑在 5173（Vite），而**网关**在 18788。用户在管理台点
	// 「导入 CC Switch」时，如果地址填的是 5173，CC Switch 会去打前端开发服务器，
	// 得到 404 —— 看起来像"网关坏了"，其实是地址指错了进程。
	//
	// 无需鉴权：这些地址本来就印在用户自己的客户端配置里，不是秘密。
	// 但**不包含任何密钥**。
	mux.HandleFunc("GET /public-config", func(w http.ResponseWriter, r *http.Request) {
		httpx.OK(w, map[string]any{
			"gatewayBaseUrl": ctx.Gateway.PublicBaseURL(),
			"siteName":       "relayAI",
		})
	})
}

// serviceVersion 与 backend.api 的 info.version 保持一致。
const serviceVersion = "v0.3.0"
