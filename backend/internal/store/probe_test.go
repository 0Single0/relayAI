package store_test

import (
	"context"
	"testing"

	"relayai/backend/internal/models"
	"relayai/backend/internal/store"
)

/*
探活目标的选取规则。

这几条规则决定了"定时探活探什么"，错了会直接表现为
"某个模型挂了但界面全绿"或者"白白多烧几倍上游额度"。
*/

// seedChannelWithAccounts 建一个分组和若干个账号。
func seedChannelWithAccounts(
	t *testing.T,
	s *store.Store,
	channelID string,
	uids []string,
	modelAllow []string,
) {
	t.Helper()
	ctx := context.Background()

	enabled := true
	if _, err := s.CreateChannel(ctx, store.ChannelInput{
		ID:         channelID,
		Name:       channelID,
		Adapter:    "custom",
		Enabled:    &enabled,
		ModelAllow: modelAllow,
	}); err != nil {
		t.Fatalf("建分组失败: %v", err)
	}
	for _, uid := range uids {
		if _, err := s.CreateAccount(ctx, channelID, uid, uid, "cred"); err != nil {
			t.Fatalf("建账号失败: %v", err)
		}
	}
}

/*
	没配 probe_models 的分组**一条目标都不产生**。

	这是刻意的：探活消耗真实额度，"没有人说要探"绝不能变成"探全部"。
	一次目录同步进来几十个模型，自动全探等于在后台悄悄烧钱 ——
	实测就是这里曾经回退到 exposed 全集，用户配了 2 个却跑了 19 个。
*/
func TestProbeTargetsSkipsChannelWithoutProbeModels(t *testing.T) {
	s, _ := testDB(t)
	seedChannelWithAccounts(t, s, "ch-a", []string{"u1"}, []string{"m1", "m2", "m3"})

	targets, err := s.ProbeTargets(context.Background())
	if err != nil {
		t.Fatalf("查询探活目标失败: %v", err)
	}
	if len(targets) != 0 {
		t.Fatalf("没配 probe_models 的分组不应产生任何目标，得到 %+v", targets)
	}
}

// 配了 probe_models 就只探那些，不探全部。
func TestProbeTargetsHonoursProbeModels(t *testing.T) {
	s, _ := testDB(t)
	seedChannelWithAccounts(t, s, "ch-b", []string{"u1"}, []string{"m1", "m2", "m3"})

	// 只探 m2。
	probeModels := []string{"m2"}
	if _, err := s.UpdateChannel(context.Background(), "ch-b", store.ChannelInput{
		ProbeModels: &probeModels,
	}); err != nil {
		t.Fatalf("设置 probeModels 失败: %v", err)
	}

	targets, err := s.ProbeTargets(context.Background())
	if err != nil {
		t.Fatalf("查询探活目标失败: %v", err)
	}
	if len(targets) != 1 || targets[0].Model != "m2" {
		t.Fatalf("应只探 m2，得到 %+v", targets)
	}
}

/*
同一模型只挑**一个**账号。

探活回答的是"这个分组能不能用这个模型"。
一个分组 20 个号、10 个模型，逐个都探就是 200 次上游请求 ——
那是把额度当柴烧，而且结果完全重复。
*/
func TestProbeTargetsPicksOneAccountPerModel(t *testing.T) {
	s, _ := testDB(t)
	seedChannelWithAccounts(t, s, "ch-c", []string{"u1", "u2", "u3"}, []string{"m1"})

	// 必须先显式配置 probe_models —— 不配就不探，这是刻意的。
	probeModels := []string{"m1"}
	if _, err := s.UpdateChannel(context.Background(), "ch-c", store.ChannelInput{
		ProbeModels: &probeModels,
	}); err != nil {
		t.Fatalf("设置 probeModels 失败: %v", err)
	}

	targets, err := s.ProbeTargets(context.Background())
	if err != nil {
		t.Fatalf("查询探活目标失败: %v", err)
	}
	if len(targets) != 1 {
		t.Fatalf("1 个模型应只产生 1 条目标，得到 %d 条：%+v", len(targets), targets)
	}
	if targets[0].AccountUID == "" {
		t.Error("必须挑出一个账号，不能为空")
	}
}

// 没有账号的分组不产生目标：没号可探，硬探只会拿到"账号不存在"。
func TestProbeTargetsSkipsChannelWithoutAccounts(t *testing.T) {
	s, _ := testDB(t)
	seedChannelWithAccounts(t, s, "ch-empty", nil, []string{"m1"})

	targets, err := s.ProbeTargets(context.Background())
	if err != nil {
		t.Fatalf("查询探活目标失败: %v", err)
	}
	if len(targets) != 0 {
		t.Fatalf("没有账号的分组不该产生目标，得到 %+v", targets)
	}
}

// 保存与读取：最新一条要能取回来，历史按时间倒序。
func TestSaveAndReadProbes(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	for _, item := range []models.ChannelProbes{
		{ChannelID: "ch", Model: "m1", OK: true, Status: "ok", LatencyMS: 100, Trigger: "schedule"},
		{ChannelID: "ch", Model: "m1", OK: false, Status: "limited", LatencyMS: 0, Message: "限流", Trigger: "schedule"},
	} {
		if err := s.SaveProbe(ctx, item); err != nil {
			t.Fatalf("保存探活失败: %v", err)
		}
	}

	latest, err := s.LatestProbes(ctx)
	if err != nil {
		t.Fatalf("查询最新探活失败: %v", err)
	}
	if len(latest) != 1 {
		t.Fatalf("同一 (分组,模型) 只应返回最新一条，得到 %d 条", len(latest))
	}
	if latest[0].Status != "limited" {
		t.Errorf("最新一条应是 limited，得到 %s", latest[0].Status)
	}

	history, err := s.ProbeHistory(ctx, "ch", 10)
	if err != nil {
		t.Fatalf("查询历史失败: %v", err)
	}
	if len(history) != 2 {
		t.Fatalf("历史应有 2 条，得到 %d 条", len(history))
	}
}
