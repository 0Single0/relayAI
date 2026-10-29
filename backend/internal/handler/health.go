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
		// GORM 没有 Ping，要拿到底层 `*sql.DB` 来探活。
		if ctx.DB != nil {
			if sqlDB, err := ctx.DB.DB(); err == nil && sqlDB.PingContext(r.Context()) == nil {
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
	// Docker 通过同一个 Nginx 提供管理台与网关，用显式的 same-origin 配置
	// 允许前端使用浏览器当前地址。开发环境与独立网关仍使用配置的地址。
	//
	// 无需鉴权：这些地址本来就印在用户自己的客户端配置里，不是秘密。
	// 但**不包含任何密钥**。
	mux.HandleFunc("GET /public-config", func(w http.ResponseWriter, r *http.Request) {
		baseURL := ctx.Gateway.PublicBaseURL()
		sameOrigin := baseURL == "same-origin"
		if sameOrigin {
			baseURL = ""
		}
		httpx.OK(w, map[string]any{
			"gatewayBaseUrl":    baseURL,
			"gatewaySameOrigin": sameOrigin,
			"siteName":          "relayAI",
		})
	})
}

// serviceVersion 与 backend.api 的 info.version 保持一致。
const serviceVersion = "v0.3.0"
