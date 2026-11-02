package store_test

import (
	"context"
	"testing"
	"time"

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

/*
「分组状态」卡片上的红字只描述**此刻**，不是历史账本。

真实的坑：一条两周前的 401 会永远挂在卡片上，而旁边细条连绿、可用性 99%。
用户看到"刚刚探活成功"和"401"并排，只能理解成系统坏了，真正的线索被淹没。

所以口径是：最近一条探活失败 → 显示原因；最近一条成功 → 不显示。
历史失败仍然留在表里（`ProbeHistory` 能查到），只是不再冒充当前故障。
*/
func TestLastProbeErrorClearsAfterRecovery(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	visible := true
	if _, err := s.CreateChannel(ctx, store.ChannelInput{
		ID:            "ch-flaky",
		Name:          "ch-flaky",
		Adapter:       "workbuddy",
		StatusVisible: &visible,
	}); err != nil {
		t.Fatalf("建分组失败: %v", err)
	}

	healthOf := func() store.ChannelHealth {
		t.Helper()
		list, err := s.ListChannelHealth(ctx, time.Time{})
		if err != nil {
			t.Fatalf("读取分组健康度失败: %v", err)
		}
		for _, item := range list {
			if item.ID == "ch-flaky" {
				return item
			}
		}
		t.Fatal("分组不在健康度列表里")
		return store.ChannelHealth{}
	}

	// 1) 只有一条失败：红字要出现，并带上发生时刻。
	if err := s.SaveProbe(ctx, models.ChannelProbes{
		ChannelID: "ch-flaky", Model: "m1", OK: false, Status: "auth",
		Message: "401 Authorization Required", Trigger: "schedule",
	}); err != nil {
		t.Fatalf("保存探活失败: %v", err)
	}
	failed := healthOf()
	if failed.LastError == nil || *failed.LastError != "401 Authorization Required" {
		t.Fatalf("最近一条失败时应给出原因，得到 %v", failed.LastError)
	}
	if failed.LastErrorAt == nil || *failed.LastErrorAt == "" {
		t.Error("失败原因必须带发生时刻，否则用户无法判断它有多旧")
	}

	// 2) 之后成功一次：旧 401 必须消失（这才是"一直都有 401"的根因）。
	if err := s.SaveProbe(ctx, models.ChannelProbes{
		ChannelID: "ch-flaky", Model: "m1", OK: true, Status: "ok",
		LatencyMS: 120, Trigger: "schedule",
	}); err != nil {
		t.Fatalf("保存探活失败: %v", err)
	}
	recovered := healthOf()
	if recovered.LastError != nil {
		t.Errorf("最近一条成功时不该继续挂旧错误，得到 %q", *recovered.LastError)
	}
	if recovered.LastErrorAt != nil {
		t.Errorf("错误消失时时刻也要跟着清空，得到 %q", *recovered.LastErrorAt)
	}
	// 历史不能被删：排障还要能翻到那条 401。
	history, err := s.ProbeHistory(ctx, "ch-flaky", 10)
	if err != nil {
		t.Fatalf("查询历史失败: %v", err)
	}
	if len(history) != 2 {
		t.Errorf("历史失败记录应保留，期望 2 条，得到 %d 条", len(history))
	}
}
