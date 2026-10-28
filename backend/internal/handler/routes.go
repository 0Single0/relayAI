package handler

import (
	"log"
	"net/http"

	"relayai/backend/internal/svc"
)

// RegisterHandlers 注册全部管理面路由，返回可挂到任意 http.Server 的 Handler。
//
// 为什么不用 go-zero 的 rest.Server：
//   1. 它不支持 PATCH（本 API 有大量部分更新）；
//   2. 它的路由不支持 `{id}` 路径参数（只能 `:id` 前缀匹配）；
//   3. 与标准库 mux 混用会出现前缀与中间件两套语义。
//
// 所以改用标准库 `http.ServeMux`（Go 1.22+ 支持方法 + 路径参数），
// go-zero 只保留配置加载与日志（见 backend.go）。
//
// 路径与方法遵循 REST 惯例（SPEC 7.2）：
//   - 集合用复数：/channels、/accounts、/keys
//   - 单条用路径参数：/channels/{id}
//   - 动作型接口用子资源 + POST：/channels/{id}/probe、/tasks/runs/{id}/cancel
func RegisterHandlers(ctx *svc.ServiceContext) http.Handler {
	mux := http.NewServeMux()
	registerHealth(mux, ctx)
	registerAuth(mux, ctx)
	registerChannels(mux, ctx)
	registerAccounts(mux, ctx)
	registerKeys(mux, ctx)
	registerProxies(mux, ctx)
	registerModels(mux, ctx)
	registerCatalog(mux, ctx)
	registerPricing(mux, ctx)
	registerSettlement(mux, ctx)
	registerOps(mux, ctx)
	registerUsage(mux, ctx)
	registerStats(mux, ctx)
	registerTasks(mux, ctx)
	registerTaskStream(mux, ctx)
	registerSystem(mux, ctx)

	return withMiddleware(http.StripPrefix("/api/v1", mux))
}

// withMiddleware 包一层：请求日志 + panic 恢复。
func withMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		defer func() {
			if recovered := recover(); recovered != nil {
				log.Printf("[backend] panic: %v (%s %s)", recovered, r.Method, r.URL.Path)
				http.Error(w, `{"code":500,"success":false,"msg":"服务内部错误"}`, http.StatusInternalServerError)
			}
		}()

		next.ServeHTTP(w, r)
	})
}
