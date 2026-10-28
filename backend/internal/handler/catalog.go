package handler

import (
	"context"
	"encoding/json"
	"net/http"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerCatalog 注册模型目录同步。
//
// 为什么需要"同步"这一步：模型清单**只存在于网关的内存矩阵里**
// （从各渠道适配器实时拉取），从来没有落进 `models` 表。
// 而管理面的模型页与公开的模型广场都读那张表，于是永远是空的。
// 所以必须由管理面主动把目录拉过来写库。
func registerCatalog(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("POST /catalog/sync", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		result, err := SyncCatalog(r.Context(), ctx)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		// 同步可能带进新模型；价格表要跟着推一次，
		// 否则新模型在网关侧没有价格，成本会算成 0。
		pushPrices(r, ctx)
		httpx.OK(w, result)
	})

	// 模型 × 渠道 视图（管理面）。
	//
	// 为什么按渠道分组：同一个模型在不同渠道下的**价格与可用性**都可能不同
	// （渠道倍率不同、有的渠道根本不提供它）。平铺成一张表看不出这件事，
	// 用户会以为"这个模型到处都能用、到处一个价"。
	mux.HandleFunc("GET /catalog/models", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		models, err := ctx.Store.ListModels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		channels, err := ctx.Store.ListChannels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 网关目录里带「模型 → 渠道」路由，用它判断某模型在哪些渠道可用。
		//
		// **注意**：网关的 `channelId` 实际是**平台名**（`workbuddy` / `trae`），
		// 不是渠道 id（`wb-cn`）。平台下沉到账号之后，注册表的键换成了平台，
		// 目录也跟着用平台 —— 直接用它会一个渠道都对不上，
		// 界面就会把所有模型标成"未分配到渠道"。
		// 所以这里用 `accounts` 表做一次「平台 → 渠道」反查。
		routes, err := ctx.Gateway.GetCatalog(r.Context())
		if err != nil {
			// 网关不可用时降级：不给渠道归属，但价格仍可看。
			// 这比整页报错有用 —— 用户多半是来改价格的。
			routes = nil
		}

		accounts, _, err := ctx.Store.ListAccounts(r.Context(), store.AccountQuery{
			Page:     1,
			PageSize: 1000,
		})
		if err != nil {
			accounts = nil
		}

		// 平台 → 渠道 id 集合（同一平台可能对应多个渠道）。
		channelsByPlatform := make(map[string][]string, len(accounts))
		for _, account := range accounts {
			list := channelsByPlatform[account.Platform]
			already := false
			for _, id := range list {
				if id == account.ChannelID {
					already = true
					break
				}
			}
			if !already {
				channelsByPlatform[account.Platform] = append(list, account.ChannelID)
			}
		}

		byModel := make(map[string][]string, len(routes))
		for _, item := range routes {
			for _, channel := range item.Channels {
				// 先按平台反查真实渠道；查不到就原样保留
				// （可能是 fake 这类没有对应渠道记录的平台）。
				ids := channelsByPlatform[channel.ChannelID]
				if len(ids) == 0 {
					ids = []string{channel.ChannelID}
				}
				for _, id := range ids {
					byModel[item.Name] = append(byModel[item.Name], id)
				}
			}
		}

		type channelRef struct {
			ID    string  `json:"id"`
			Name  string  `json:"name"`
			Ratio float64 `json:"ratio"`
			// Exposed 表示该分组**对外提供**这个模型。
			//
			// 上游技术上能跑不等于愿意卖：分组可以配 `model_allow` 只开放一部分。
			// 保留 `false` 的条目（而不是直接过滤掉）是为了让管理员看到
			// "这个号能跑、但被分组的白名单挡住了" —— 直接过滤会让人以为
			// 上游根本不支持，排查方向就完全错了。
			Exposed bool `json:"exposed"`
		}

		type modelView struct {
			Name           string          `json:"name"`
			DisplayName    string          `json:"displayName"`
			ContextWindow  *int            `json:"contextWindow"`
			MaxOutput      *int            `json:"maxOutput"`
			Capabilities   json.RawMessage `json:"capabilities"`
			InputPrice     float64         `json:"inputPrice"`
			OutputPrice    float64         `json:"outputPrice"`
			CacheReadPrice float64         `json:"cacheReadPrice"`
			ModelRatio     float64         `json:"modelRatio"`
			Enabled        bool            `json:"enabled"`
			Priced         bool            `json:"priced"`
			// Channels 是该模型可用的渠道（含各自倍率）。
			Channels []channelRef `json:"channels"`
		}

		channelByID := make(map[string]store.Channel, len(channels))
		for _, channel := range channels {
			channelByID[channel.ID] = channel
		}

		out := make([]modelView, 0, len(models))
		for _, model := range models {
			refs := make([]channelRef, 0, 4)
			for _, id := range byModel[model.CanonicalName] {
				channel, ok := channelByID[id]
				if !ok {
					continue
				}
				refs = append(refs, channelRef{
					ID:      channel.ID,
					Name:    channel.Name,
					Ratio:   channel.ChannelRatio,
					Exposed: channelModelAllowed(channel, model.CanonicalName),
				})
			}

			out = append(out, modelView{
				Name:           model.CanonicalName,
				DisplayName:    model.DisplayName,
				ContextWindow:  model.ContextWindow,
				MaxOutput:      model.MaxOutput,
				Capabilities:   model.Capabilities,
				InputPrice:     model.InputPrice,
				OutputPrice:    model.OutputPrice,
				CacheReadPrice: model.CacheReadPrice,
				ModelRatio:     model.ModelRatio,
				Enabled:        model.Enabled,
				Priced:         model.InputPrice > 0 || model.OutputPrice > 0,
				Channels:       refs,
			})
		}

		httpx.OK(w, map[string]any{"models": out})
	})
}

// gatewayRouteIndex 把网关目录转成「模型对外名 → 分组 id 列表」。
//
// **网关的 `channelId` 实际是平台名**（`workbuddy` / `trae`），不是分组 id（`wb-cn`）——
// 平台下沉到账号之后，注册表的键换成了平台，目录也跟着用平台。
// 直接用它会一个分组都对不上，界面就会把所有模型标成"未对外开放"。
// 所以要用 `accounts` 表做一次「平台 → 分组」反查。
//
// 返回 nil 表示网关目录不可用（降级：调用方应当放宽判断，不是全部拒绝）。
func gatewayRouteIndex(
	ctx context.Context,
	svcCtx *svc.ServiceContext,
) map[string][]string {
	routes, err := svcCtx.Gateway.GetCatalog(ctx)
	if err != nil {
		return nil
	}

	accounts, _, err := svcCtx.Store.ListAccounts(ctx, store.AccountQuery{
		Page:     1,
		PageSize: 1000,
	})
	if err != nil {
		accounts = nil
	}

	// 平台 → 分组 id 集合（同一平台可能对应多个分组）。
	channelsByPlatform := make(map[string][]string, len(accounts))
	for _, account := range accounts {
		list := channelsByPlatform[account.Platform]
		already := false
		for _, id := range list {
			if id == account.ChannelID {
				already = true
				break
			}
		}
		if !already {
			channelsByPlatform[account.Platform] = append(channelsByPlatform[account.Platform], account.ChannelID)
		}
	}

	out := make(map[string][]string, len(routes))
	for _, item := range routes {
		for _, channel := range item.Channels {
			ids := channelsByPlatform[channel.ChannelID]
			if len(ids) == 0 {
				// 查不到映射（如 fake 这类没有对应分组记录的平台）：原样保留。
				ids = []string{channel.ChannelID}
			}
			out[item.Name] = append(out[item.Name], ids...)
		}
	}
	return out
}


type SyncResult struct {
	// Synced 是写入/更新的模型数。
	Synced int `json:"synced"`
	// Removed 是被清理的下线模型数。
	Removed int64 `json:"removed"`
	// Skipped 是网关未返回任何模型时的标记（此时不会清理任何东西）。
	Skipped bool `json:"skipped"`
}

// SyncCatalog 把网关的模型目录同步进 `models` 表。
//
// 三件事：
//   1. 拉网关的 `/internal/catalog`；
//   2. 逐个 upsert（只更新目录字段，价格与倍率保持人工配置）；
//   3. 删掉不在清单里的模型（上游已下线）。
//
// 网关返回空清单时**跳过清理**：那通常意味着网关刚重启、矩阵还没建好，
// 此时清空整张表会让用户以为模型全没了。
func SyncCatalog(ctx context.Context, svcCtx *svc.ServiceContext) (*SyncResult, error) {
	models, err := svcCtx.Gateway.GetCatalog(ctx)
	if err != nil {
		return nil, err
	}

	result := &SyncResult{Skipped: len(models) == 0}
	if result.Skipped {
		return result, nil
	}

	keep := make([]string, 0, len(models))
	for _, item := range models {
		capsJSON, _ := json.Marshal(capabilitiesOf(item.Capabilities))

		// 上下文：取矩阵里最大的那个档位。
		var contextWindow *int
		for _, value := range item.ContextWindows {
			candidate := int(value)
			if contextWindow == nil || candidate > *contextWindow {
				contextWindow = &candidate
			}
		}

		var maxOutput *int
		if item.MaxOutput != nil {
			value := int(*item.MaxOutput)
			maxOutput = &value
		}

		if err := svcCtx.Store.UpsertCatalogModel(ctx, store.CatalogModelInput{
			CanonicalName: item.Name,
			// 没有单独的展示名时用对外名，别留空 —— 界面会显示成空白标题。
			DisplayName:   item.Name,
			ContextWindow: contextWindow,
			MaxOutput:     maxOutput,
			Capabilities:  capsJSON,
		}); err != nil {
			return nil, err
		}
		keep = append(keep, item.Name)
	}

	removed, err := svcCtx.Store.DeleteModelsExcept(ctx, keep)
	if err != nil {
		return nil, err
	}

	result.Synced = len(models)
	result.Removed = removed
	logx.Infof("模型目录已同步：%d 个，清理 %d 个", result.Synced, result.Removed)
	return result, nil
}

// capabilitiesOf 把网关的能力字段翻成落库的 JSON。
//
// 两条规则：
//  1. 只写**明确为真**的能力位 —— 键不存在表示"没有这个能力"，键为 false
//     与"没这条信息"在界面上要能分开；
//  2. 来源（live / snapshot / none）原样透传，不压成布尔。
//
// 第 2 条是改版的重点：早先只写 `capabilitiesKnown = false`，把"上游没声明"
// 与"我们按内置对照表填的"混成一档，于是卡片上出现「工具调用 视觉 推理」
// 与「能力未知」并列 —— 看起来像自相矛盾。
func capabilitiesOf(capabilities gateway.CatalogCapabilities) map[string]any {
	caps := map[string]any{}
	if capabilities.Vision {
		caps["vision"] = true
	}
	if capabilities.Tools {
		caps["tools"] = true
	}
	if capabilities.Reasoning {
		caps["reasoning"] = true
	}
	switch capabilities.Source {
	case "live", "snapshot", "none":
		caps["capabilitiesSource"] = capabilities.Source
	}
	return caps
}