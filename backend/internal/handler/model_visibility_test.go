package handler

import (
	"testing"

	"relayai/backend/internal/store"
)

// 分组白名单的过滤语义，逐条锁死。
//
// 这些断言直接对应"管理员把 WorkBuddy 限制成只用 ds 和 glm 之后，
// 模型广场只剩那几个"这条需求 —— 改坏任何一条都会让需求失效。
func TestChannelModelAllowed(t *testing.T) {
	base := store.Channel{ID: "wb-cn", Enabled: true}

	t.Run("白名单为空表示不限制", func(t *testing.T) {
		if !channelModelAllowed(base, "anything") {
			t.Fatal("空白名单应当放行全部模型")
		}
	})

	t.Run("白名单有值时只放行列内的", func(t *testing.T) {
		channel := base
		channel.ModelAllow = []string{"deepseek-v4-pro", "glm-5.1"}

		if !channelModelAllowed(channel, "deepseek-v4-pro") {
			t.Fatal("列表内的模型应当放行")
		}
		if channelModelAllowed(channel, "glm-5.2") {
			t.Fatal("列表外的模型不该放行")
		}
	})

	t.Run("停用的分组一个都不放行", func(t *testing.T) {
		channel := store.Channel{ID: "wb-cn", Enabled: false}
		if channelModelAllowed(channel, "anything") {
			t.Fatal("停用的分组不该提供任何模型")
		}
	})
}

func TestVisibleModels(t *testing.T) {
	models := []store.Model{
		{CanonicalName: "deepseek-v4-pro", Enabled: true, InputPrice: 10},
		{CanonicalName: "glm-5.1", Enabled: true, InputPrice: 10},
		{CanonicalName: "glm-5.2", Enabled: true, InputPrice: 10},
	}

	t.Run("只有一个分组且配了白名单时只留白名单内的", func(t *testing.T) {
		channels := []store.Channel{
			{
				ID:           "wb-cn",
				Enabled:      true,
				ChannelRatio: 1,
				ModelAllow:   []string{"deepseek-v4-pro", "glm-5.1"},
			},
		}

		routes := ModelRouteSource{
			"deepseek-v4-pro": {"wb-cn": {}},
			"glm-5.1":         {"wb-cn": {}},
			"glm-5.2":         {"wb-cn": {}},
		}
		visible := visibleModels(channels, models, routes)
		if len(visible) != 2 {
			names := make([]string, 0, len(visible))
			for _, item := range visible {
				names = append(names, item.Model.CanonicalName)
			}
			t.Fatalf("应当只剩 2 个，实际 %d：%v", len(visible), names)
		}
	})

	t.Run("另一个不受限的分组仍能让模型可见", func(t *testing.T) {
		channels := []store.Channel{
			{
				ID:           "wb-cn",
				Enabled:      true,
				ChannelRatio: 1,
				ModelAllow:   []string{"deepseek-v4-pro"},
			},
			// trae-cn 没配白名单 = 不限制，所以 glm-5.2 仍然可见。
			{ID: "trae-cn", Enabled: true, ChannelRatio: 1},
		}

		routes := ModelRouteSource{
			"deepseek-v4-pro": {"wb-cn": {}, "trae-cn": {}},
			"glm-5.1":         {"wb-cn": {}},
			"glm-5.2":         {"trae-cn": {}},
		}
		visible := visibleModels(channels, models, routes)
		// deepseek-v4-pro（两个分组都有）与 glm-5.2（trae-cn）可见；
		// glm-5.1 **只**挂在被限制的 wb-cn 下，看不到。
		if len(visible) != 2 {
			names := make([]string, 0, len(visible))
			for _, item := range visible {
				names = append(names, item.Model.CanonicalName)
			}
			t.Fatalf("应当剩 2 个，实际 %d：%v", len(visible), names)
		}
		for _, item := range visible {
			if item.Model.CanonicalName == "glm-5.1" {
				t.Fatal("glm-5.1 只挂在被限制的分组下，不该可见")
			}
		}
	})

	t.Run("停用的分组不提供模型", func(t *testing.T) {
		channels := []store.Channel{
			{ID: "wb-cn", Enabled: false, ChannelRatio: 1},
		}

		// 三个模型都挂在停用的分组下 → 一个都不该可见。
		routes := ModelRouteSource{
			"deepseek-v4-pro": {"wb-cn": {}},
			"glm-5.1":         {"wb-cn": {}},
			"glm-5.2":         {"wb-cn": {}},
		}
		if visible := visibleModels(channels, models, routes); len(visible) != 0 {
			t.Fatalf("停用的分组不该提供任何模型，实际 %d", len(visible))
		}
	})

	t.Run("价格取提供该模型的分组里最低倍率", func(t *testing.T) {
		channels := []store.Channel{
			{ID: "expensive", Enabled: true, ChannelRatio: 2},
			{ID: "cheap", Enabled: true, ChannelRatio: 0.5},
		}

		routes := ModelRouteSource{"deepseek-v4-pro": {"expensive": {}, "cheap": {}}}
		visible := visibleModels(channels, models[:1], routes)
		if len(visible) != 1 {
			t.Fatalf("应当只剩 1 个，实际 %d", len(visible))
		}
		if visible[0].Ratio != 0.5 {
			t.Fatalf("应当取最低倍率 0.5，实际 %v", visible[0].Ratio)
		}
	})
}
