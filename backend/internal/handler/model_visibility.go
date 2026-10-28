package handler

import "relayai/backend/internal/store"

// channelModelAllowed 判断某个分组是否**对外提供**某个模型。
//
// 三种情况：
//   - 分组停用 → 不提供（停用的分组不该出现在任何清单里）；
//   - `model_allow` 为空 → **不限制**，分组名下账号能跑的模型全部对外；
//   - `model_allow` 有值 → 只提供列表里的。
//
// 注意"空 = 不限制"而不是"空 = 一个都不给"：后者会让新建分组默认不可用，
// 而且界面上很难看出是"没配"还是"配成空"。
func channelModelAllowed(channel store.Channel, model string) bool {
	if !channel.Enabled {
		return false
	}
	if len(channel.ModelAllow) == 0 {
		return true
	}
	for _, allowed := range channel.ModelAllow {
		if allowed == model {
			return true
		}
	}
	return false
}

// ModelRouteSource 是「模型 → 分组」的**事实来源**：哪些分组真的有这个模型的路由。
//
// 为什么必须单独传：只靠 `channelModelAllowed` 不够 ——
// 一个没配白名单的分组会"放行全部模型"，于是任何模型都被它'提供'，
// 限制别的分组就等于没限制。必须先知道"这个分组真的有这个模型"（来自网关目录），
// 再叠加白名单判断。
//
// key = 模型对外名，value = 提供它的分组 id 集合。
type ModelRouteSource map[string]map[string]struct{}

// VisibleModel 是一个对外可见的模型（含最低倍率与提供它的分组）。
type VisibleModel struct {
	Model store.Model
	// Ratio 是提供该模型的分组里**最低**的倍率 ——
	// 用户关心的是"最便宜多少钱用上"。
	Ratio float64
	// Channels 是**实际对外提供**该模型的分组 id（去重）。
	Channels []string
}

// visibleModels 计算**对外可见**的模型清单。
//
// 一个模型对外可见，当且仅当存在一个分组同时满足：
//   1. 该分组**真的有**这个模型的路由（`routes` 里查得到）；
//   2. 该分组的外白名单放行它（`channelModelAllowed`）。
//
// 第 1 条是关键：少了它，任何"没配白名单"的分组都会放行全部模型，
// 限制别的分组就等于没限制 —— 这正是"把 WorkBuddy 限制成只用 ds/glm
// 之后模型广场还是全都有"的根因。
func visibleModels(
	channels []store.Channel,
	models []store.Model,
	routes ModelRouteSource,
) []VisibleModel {
	byName := make(map[string]*VisibleModel, len(models))
	order := make([]string, 0, len(models))

	for _, model := range models {
		if !model.Enabled {
			continue
		}

		providers := routes[model.CanonicalName]
		if len(providers) == 0 {
			// 网关目录里没有这个模型的路由（可能是历史残留）。
			// 保守起见仍然展示，但不参与"提供者"计算 —— 让管理员能看到它、
			// 从而发现"目录里有、网关没路由"这种不一致。
			byName[model.CanonicalName] = &VisibleModel{Model: model, Ratio: 1}
			order = append(order, model.CanonicalName)
			continue
		}

		for _, channel := range channels {
			if _, ok := providers[channel.ID]; !ok {
				continue
			}
			if !channelModelAllowed(channel, model.CanonicalName) {
				continue
			}

			entry, ok := byName[model.CanonicalName]
			if !ok {
				entry = &VisibleModel{Model: model, Ratio: channel.ChannelRatio}
				byName[model.CanonicalName] = entry
				order = append(order, model.CanonicalName)
			} else if channel.ChannelRatio < entry.Ratio {
				entry.Ratio = channel.ChannelRatio
			}

			duplicated := false
			for _, id := range entry.Channels {
				if id == channel.ID {
					duplicated = true
					break
				}
			}
			if !duplicated {
				entry.Channels = append(entry.Channels, channel.ID)
			}
		}
	}

	out := make([]VisibleModel, 0, len(order))
	for _, name := range order {
		entry := byName[name]
		// 有路由但没有分组对外提供（全被白名单挡住）→ 不展示。
		// 这是"限制生效"的直接体现。
		if len(entry.Channels) == 0 && len(routes[name]) > 0 {
			continue
		}
		out = append(out, *entry)
	}
	return out
}

// buildRouteSource 把网关目录转成「模型 → 分组」索引。
//
// 网关的 `channelId` 是**平台名**（`workbuddy`），要用 `accounts` 表反查真实分组 id。
// 这一步在调用方做（`catalog/models` 里已有同样的逻辑），这里只收结果。
func buildRouteSource(entries map[string][]string) ModelRouteSource {
	out := make(ModelRouteSource, len(entries))
	for model, channelIDs := range entries {
		set := make(map[string]struct{}, len(channelIDs))
		for _, id := range channelIDs {
			set[id] = struct{}{}
		}
		out[model] = set
	}
	return out
}
