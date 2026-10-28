package handler

import (
	"encoding/json"
	"net/http"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerModels 注册模型相关路由。
func registerModels(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /models", func(w http.ResponseWriter, r *http.Request) {
		list, err := ctx.Store.ListModels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, list)
	})

	// 模型广场（**公开**，无需登录）。
	//
	// 为什么公开：用户选型前要知道"这个站有哪些模型、多少钱"。
	// 藏在登录后面等于让人先注册、再发现没有自己需要的模型。
	//
	// 只暴露展示必需的信息：模型名、上下文、能力、价格、生效倍率。
	// **不暴露**内部 id、上游模型名、账号数、成功率 ——
	// 那些是运维信息，对外没有价值，反而泄露部署细节。
	mux.HandleFunc("GET /public/models", func(w http.ResponseWriter, r *http.Request) {
		list, err := ctx.Store.ListModels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		channels, err := ctx.Store.ListChannels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// **按分组的模型白名单过滤**：管理员把某个分组限制成"只对外提供
		// ds 与 glm"之后，这里就只剩那几个 —— 模型广场跟着变。
		//
		// 需要网关目录提供"哪个分组真的有哪个模型"：只靠白名单不够 ——
		// 一个没配白名单的分组会放行全部模型，限制别的分组就等于没限制。
		visible := visibleModels(
			channels,
			list,
			buildRouteSource(gatewayRouteIndex(r.Context(), ctx)),
		)

		type publicModel struct {
			Name          string          `json:"name"`
			DisplayName   string          `json:"displayName"`
			ContextWindow *int            `json:"contextWindow"`
			MaxOutput     *int            `json:"maxOutput"`
			Capabilities  json.RawMessage `json:"capabilities"`
			// 价格单位：元 / 百万 token，已乘倍率。
			InputPrice  float64 `json:"inputPrice"`
			OutputPrice float64 `json:"outputPrice"`
			// CacheReadPrice 是缓存读取价；0 表示未配置（按输入价计）。
			CacheReadPrice float64 `json:"cacheReadPrice"`
			// Priced 表示是否配过价。未定价时前端要显式提示，
			// 否则"0 元"会被读成"免费"。
			Priced bool `json:"priced"`
		}

		out := make([]publicModel, 0, len(visible))
		lowestRatio := 1.0
		for index, entry := range visible {
			model := entry.Model
			if index == 0 || entry.Ratio < lowestRatio {
				lowestRatio = entry.Ratio
			}
			out = append(out, publicModel{
				Name:           model.CanonicalName,
				DisplayName:    model.DisplayName,
				ContextWindow:  model.ContextWindow,
				MaxOutput:      model.MaxOutput,
				Capabilities:   model.Capabilities,
				InputPrice:     model.InputPrice * entry.Ratio,
				OutputPrice:    model.OutputPrice * entry.Ratio,
				CacheReadPrice: model.CacheReadPrice * entry.Ratio,
				Priced:         model.InputPrice > 0 || model.OutputPrice > 0,
			})
		}

		httpx.OK(w, map[string]any{
			"models": out,
			// 让页面能说明"价格按哪个倍率算的"。
			"ratio": lowestRatio,
		})
	})

	mux.HandleFunc("PATCH /models/{name}", func(w http.ResponseWriter, r *http.Request) {
		var input store.ModelInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.InputPrice != nil {
			logx.Infof("[debug] 解析到 inputPrice=%v outputPrice=%v cache=%v",
				*input.InputPrice,
				func() any { if input.OutputPrice == nil { return nil }; return *input.OutputPrice }(),
				func() any { if input.CacheReadPrice == nil { return nil }; return *input.CacheReadPrice }(),
			)
		}
		model, err := ctx.Store.UpdateModel(r.Context(), r.PathValue("name"), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		// 改了价格必须推给网关：价格是它内存里的一份副本，
		// 不推的话它会继续按旧价记账 —— 那种错误不报错、只让账单慢慢偏掉。
		pushPrices(r, ctx)
		httpx.OK(w, model)
	})
}

// registerUsage 注册用量相关路由。
func registerUsage(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /usage", func(w http.ResponseWriter, r *http.Request) {
		query := store.UsageQuery{
			From:       r.URL.Query().Get("from"),
			To:         r.URL.Query().Get("to"),
			ChannelID:  r.URL.Query().Get("channelId"),
			AccountUID: r.URL.Query().Get("accountUid"),
			KeyID:      httpx.Int64Query(r, "keyId"),
			Model:      r.URL.Query().Get("model"),
			Status:     r.URL.Query().Get("status"),
			Page:       httpx.IntQuery(r, "page", 1),
			PageSize:   httpx.IntQuery(r, "pageSize", 50),
		}

		list, total, err := ctx.Store.ListUsage(r.Context(), query)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, httpx.Page[store.UsageLog]{
			List:     list,
			Total:    total,
			Page:     query.Page,
			PageSize: query.PageSize,
		})
	})

	mux.HandleFunc("GET /usage/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "记录")
		if !ok {
			return
		}

		log, err := ctx.Store.GetUsage(r.Context(), id)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, log)
	})

	mux.HandleFunc("GET /usage/summary", func(w http.ResponseWriter, r *http.Request) {
		query := store.SummaryQuery{
			From:    r.URL.Query().Get("from"),
			To:      r.URL.Query().Get("to"),
			GroupBy: r.URL.Query().Get("groupBy"),
		}

		totals, buckets, err := ctx.Store.GetUsageSummary(r.Context(), query)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{
			"totals":  totals,
			"buckets": buckets,
		})
	})

	mux.HandleFunc("GET /usage/series", func(w http.ResponseWriter, r *http.Request) {
		query := store.SeriesQuery{
			From:   r.URL.Query().Get("from"),
			To:     r.URL.Query().Get("to"),
			Model:  r.URL.Query().Get("model"),
			Bucket: r.URL.Query().Get("bucket"),
		}

		points, err := ctx.Store.GetUsageSeries(r.Context(), query)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, points)
	})
}