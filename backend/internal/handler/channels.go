package handler

import (
	"context"
	"net/http"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerChannels 注册渠道相关路由。
func registerChannels(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /channels", func(w http.ResponseWriter, r *http.Request) {
		list, err := ctx.Store.ListChannels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, list)
	})

	mux.HandleFunc("GET /channels/{id}", func(w http.ResponseWriter, r *http.Request) {
		channel, err := ctx.Store.GetChannel(r.Context(), r.PathValue("id"))
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, channel)
	})

	mux.HandleFunc("POST /channels", func(w http.ResponseWriter, r *http.Request) {
		var input store.ChannelInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.ID == "" || input.Name == "" || input.Adapter == "" {
			httpx.Fail(w, http.StatusBadRequest, "渠道 id、名称与类型都不能为空")
			return
		}

		channel, err := ctx.Store.CreateChannel(r.Context(), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 新渠道要立刻在网关里生效（否则要等重启）。
		notifyReload(r, ctx)

		httpx.OK(w, channel)
	})

	mux.HandleFunc("PATCH /channels/{id}", func(w http.ResponseWriter, r *http.Request) {
		var input store.ChannelInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		channel, err := ctx.Store.UpdateChannel(r.Context(), r.PathValue("id"), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)

		httpx.OK(w, channel)
	})

	mux.HandleFunc("DELETE /channels/{id}", func(w http.ResponseWriter, r *http.Request) {
		if err := ctx.Store.DeleteChannel(r.Context(), r.PathValue("id")); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)

		httpx.OKMessage(w, "渠道已删除")
	})

	// 探活：代理到网关（必须贴着适配器）。
	mux.HandleFunc("POST /channels/{id}/probe", func(w http.ResponseWriter, r *http.Request) {
		result, err := ctx.Gateway.ProbeChannel(r.Context(), r.PathValue("id"))
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, result)
	})

	// 模型目录：代理到网关。
	mux.HandleFunc("GET /channels/{id}/models", func(w http.ResponseWriter, r *http.Request) {
		models, err := ctx.Gateway.GetChannelModels(r.Context(), r.PathValue("id"))
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 转成前端要的形状：对外 id + 上游 id + 能力。
		list := make([]map[string]any, 0, len(models))
		for _, model := range models {
			list = append(list, map[string]any{
				"id":            model.ID,
				"upstreamId":    model.ID,
				"contextWindow": model.ContextLength,
				"maxOutput":     model.MaxOutputToken,
				"caps": map[string]any{
					"vision":    model.Capabilities.Vision,
					"toolCalls": model.Capabilities.ToolCalls,
					"reasoning": model.Capabilities.Reasoning,
				},
				"source": "live",
			})
		}

		httpx.OK(w, list)
	})
}

// notifyReload 通知网关重新加载配置。
//
// 失败只记日志：渠道已经写进库了，网关会在下次启动时读到；
// 这里不该因为网关暂时不可用而让用户的保存操作报错。
//
// **顺带推一次价格表**：网关不查 `models` 表，价格是它内存里的一份副本。
// 只 reload 不推价，会出现"改了价格但账还是按旧价算"——
// 那种错误不会报错、只会让账单慢慢偏掉，比直接失败更难发现。
func notifyReload(r *http.Request, ctx *svc.ServiceContext) {
	if err := ctx.Gateway.Reload(r.Context()); err != nil {
		logReloadFailure(r, err)
	}
	pushPrices(r, ctx)
}

// pushPrices 把「模型 → 单价」推给网关。
func pushPrices(r *http.Request, ctx *svc.ServiceContext) {
	PushPrices(r.Context(), ctx)
}

// PushPrices 把「模型 → 单价」推给网关（不依赖 HTTP 请求）。
//
// 独立出来是给后台任务用的：网关的 PriceBook 在**内存**里，
// 进程重启就空了。不主动推的话，重启后的所有请求成本都会算成 0，
// 而且价格快照也是空的 —— 事后完全查不出"这段时间为什么没计费"。
func PushPrices(ctx context.Context, svcCtx *svc.ServiceContext) {
	prices, err := svcCtx.Store.ModelPricesForGateway(ctx)
	if err != nil {
		logx.WithContext(ctx).Infof("读取模型价格失败: %v", err)
		return
	}

	entries := make([]gateway.PriceEntry, 0, len(prices))
	for model, price := range prices {
		entries = append(entries, gateway.PriceEntry{
			Model:     model,
			Input:     price.Input,
			Output:    price.Output,
			CacheRead: price.CacheRead,
		})
	}

	if err := svcCtx.Gateway.SetPrices(ctx, entries); err != nil {
		logx.WithContext(ctx).Infof("推送模型价格失败: %v", err)
		return
	}
	logx.WithContext(ctx).Infof("已推送 %d 个模型价格到网关", len(entries))
}

// pushPricesRequest 是旧入口，保留给"改价后立即生效"的调用点。
func pushPricesRequest(r *http.Request, ctx *svc.ServiceContext) {
	prices, err := ctx.Store.ModelPricesForGateway(r.Context())
	if err != nil {
		logReloadFailure(r, err)
		return
	}

	entries := make([]gateway.PriceEntry, 0, len(prices))
	for model, price := range prices {
		entries = append(entries, gateway.PriceEntry{
			Model:     model,
			Input:     price.Input,
			Output:    price.Output,
			CacheRead: price.CacheRead,
		})
	}

	if err := ctx.Gateway.SetPrices(r.Context(), entries); err != nil {
		logReloadFailure(r, err)
	}
}
