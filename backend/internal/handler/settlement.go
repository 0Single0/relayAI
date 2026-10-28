package handler

import (
	"context"
	"net/http"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"
)

// registerSettlement 注册结算相关路由。
//
// 结算 = 把 `usage_logs` 里还没扣过钱的用量，按 Key 归到用户，
// 从余额扣掉并写流水。网关只负责算钱和记用量，账务在管理面。
func registerSettlement(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("POST /settlements/run", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		limit := httpx.IntQuery(r, "limit", 500)
		result, err := ctx.Store.SettleUsage(r.Context(), limit)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		logx.WithContext(r.Context()).Infof(
			"结算完成：%d 条用量，%d 个用户，金额 %.6f",
			result.Count, result.Users, result.Amount,
		)
		httpx.OK(w, result)
	})
}

// RunSettlement 供后台定时任务调用。
//
// 为什么要有定时：结算不该依赖"管理员想起来点一下"。
// 用量一直在产生，靠手动结算意味着余额长期是旧值。
func RunSettlement(ctx *svc.ServiceContext) {
	result, err := ctx.Store.SettleUsage(context.Background(), 500)
	if err != nil {
		logx.Errorf("定时结算失败: %v", err)
		return
	}
	if result.Count > 0 {
		logx.Infof(
			"定时结算：%d 条用量，%d 个用户，扣款 %.6f 元",
			result.Count, result.Users, result.Amount,
		)
	}
}
