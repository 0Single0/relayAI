package store_test

import (
	"context"
	"os"
	"testing"
	"time"

	"relayai/backend/internal/database"
	"relayai/backend/internal/store"

	"gorm.io/gorm"
)

/*
store 层的集成测试。

为什么是集成测试而不是单元测试：
  这一层几乎全部逻辑都在 SQL 里（条件更新、聚合口径、幂等 upsert）。
  mock 掉数据库只能验证"调用了正确的 SQL 字符串"，而真正会出错的
  是 SQL 语义 —— 那必须在真库上跑。

需要 `TEST_DATABASE_URL`；没设就跳过，保持 `go test ./...` 在无库环境可用。
*/

func testDB(t *testing.T) (*store.Store, *gorm.DB) {
	t.Helper()

	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		t.Skip("未设置 TEST_DATABASE_URL，跳过 store 集成测试")
	}

	ctx := context.Background()
	db, err := database.Connect(ctx, dsn, database.DefaultOptions())
	if err != nil {
		t.Fatalf("连接测试库失败: %v", err)
	}

	// 每个测试从空 schema 开始，避免上一个用例的残留影响断言。
	if err := db.Exec("DROP SCHEMA public CASCADE").Error; err != nil {
		t.Fatalf("清空 schema 失败: %v", err)
	}
	if err := db.Exec("CREATE SCHEMA public").Error; err != nil {
		t.Fatalf("重建 schema 失败: %v", err)
	}
	if err := database.Migrate(ctx, db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	return store.New(db), db
}

func TestChannelCRUD(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	enabled := true
	ratio := 1.5
	created, err := s.CreateChannel(ctx, store.ChannelInput{
		ID:           "wb-test",
		Name:         "测试分组",
		Adapter:      "workbuddy",
		Enabled:      &enabled,
		ChannelRatio: &ratio,
		Options:      []byte(`{"realm":"cn"}`),
	})
	if err != nil {
		t.Fatalf("新建分组失败: %v", err)
	}
	if created.ChannelRatio != 1.5 {
		t.Errorf("倍率应保留小数，得到 %v", created.ChannelRatio)
	}
	// 能力缓存应随分组一起创建（workbuddy + cn → 任务/签到/每日全开）。
	if !created.Caps.Tasks {
		t.Error("国内版 workbuddy 应具备任务能力")
	}

	// 更新只传部分字段，其它字段不能被清空。
	name := "改名后"
	updated, err := s.UpdateChannel(ctx, "wb-test", store.ChannelInput{Name: name})
	if err != nil {
		t.Fatalf("更新分组失败: %v", err)
	}
	if updated.Name != name {
		t.Errorf("名称应为 %q，得到 %q", name, updated.Name)
	}
	if updated.ChannelRatio != 1.5 {
		t.Errorf("未传的倍率被清空了：%v", updated.ChannelRatio)
	}

	if err := s.DeleteChannel(ctx, "wb-test"); err != nil {
		t.Fatalf("删除分组失败: %v", err)
	}
	if _, err := s.GetChannel(ctx, "wb-test"); err != store.ErrNotFound {
		t.Errorf("删除后应返回 ErrNotFound，得到 %v", err)
	}
}

func TestChannelRatioPrecision(t *testing.T) {
	/*
		小数精度回归测试。

		历史 bug：`COALESCE(float8, 0)` 让 Postgres 把表达式推断成 integer，
		0.8 被截断成 0、2.8 变 2 —— 价格面板存进去就变回原值。
		这里用同样的形状（小于 1 的值 + 三位小数）守住它。
	*/
	s, _ := testDB(t)
	ctx := context.Background()

	want := 0.23
	created, err := s.CreateChannel(ctx, store.ChannelInput{
		ID:           "precision",
		Name:         "精度测试",
		Adapter:      "custom",
		ChannelRatio: &want,
	})
	if err != nil {
		t.Fatalf("新建失败: %v", err)
	}
	if created.ChannelRatio != want {
		t.Fatalf("倍率应为 %v，得到 %v（小数被截断）", want, created.ChannelRatio)
	}

	// 更新路径也要保精度。
	next := 0.8
	updated, err := s.UpdateChannel(ctx, "precision", store.ChannelInput{ChannelRatio: &next})
	if err != nil {
		t.Fatalf("更新失败: %v", err)
	}
	if updated.ChannelRatio != next {
		t.Fatalf("更新后倍率应为 %v，得到 %v", next, updated.ChannelRatio)
	}
}

func TestUserAndBalance(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	user, err := s.CreateUserByAdmin(ctx, store.AdminCreateUserInput{
		Email:    "test@example.com",
		Password: "secret123",
		Nickname: "测试用户",
		Role:     "admin",
		Balance:  100,
	})
	if err != nil {
		t.Fatalf("建用户失败: %v", err)
	}
	if user.Balance != 100 {
		t.Errorf("初始余额应为 100，得到 %v", user.Balance)
	}

	// 扣款：余额应减少，且写一条流水。
	adjusted, err := s.AdjustBalance(ctx, user.ID, -30, "测试扣款")
	if err != nil {
		t.Fatalf("调整余额失败: %v", err)
	}
	if adjusted.Balance != 70 {
		t.Errorf("余额应为 70，得到 %v", adjusted.Balance)
	}

	// 扣成负数必须被拒 —— 这是"不扣成负债"的硬约束。
	if _, err := s.AdjustBalance(ctx, user.ID, -1000, "超额扣款"); err != store.ErrInsufficientBalance {
		t.Errorf("超额扣款应返回 ErrInsufficientBalance，得到 %v", err)
	}

	// 邮箱唯一。
	if _, err := s.CreateUserByAdmin(ctx, store.AdminCreateUserInput{
		Email: "test@example.com", Password: "x", Nickname: "重复",
	}); err == nil {
		t.Error("重复邮箱应报错")
	}
}

/*
删分组**不能删账号**。

这是一个真实的用户投诉：「为什么我删除分组会连带着账号删除？不应该
是把账号弄成未归属状态吗」。账号是一份上游凭据（凭据、登录态、历史用量），
分组只是计价桶 —— 桶不要了，号不该跟着消失。

外键必须是 `ON DELETE SET NULL`。写成 CASCADE 时删分组会静默删掉整组账号，
而且不可撤销；这个用例就是用来守住它的。
*/
func TestDeleteChannelKeepsAccountsAsUnassigned(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	enabled := true
	if _, err := s.CreateChannel(ctx, store.ChannelInput{
		ID:      "wb-group",
		Name:    "待删除分组",
		Adapter: "workbuddy",
		Enabled: &enabled,
		Options: []byte(`{"realm":"cn"}`),
	}); err != nil {
		t.Fatalf("新建分组失败: %v", err)
	}

	created, err := s.CreateAccount(ctx, "wb-group", "uid-keep", "保留号", "")
	if err != nil {
		t.Fatalf("建号失败: %v", err)
	}
	if created.ChannelID != "wb-group" {
		t.Fatalf("账号应归入分组，得到 %q", created.ChannelID)
	}

	if err := s.DeleteChannel(ctx, "wb-group"); err != nil {
		t.Fatalf("删除分组失败: %v", err)
	}

	// 账号必须还在，且 channel_id 变成空串（待分配）。
	kept, err := s.FindAccountByUID(ctx, "", "uid-keep")
	if err != nil {
		t.Fatalf("删分组后账号不该消失: %v", err)
	}
	if kept.ChannelID != "" {
		t.Errorf("删分组后账号应回到待分配（channelId 为空），得到 %q", kept.ChannelID)
	}
	if !kept.Enabled {
		t.Error("删分组不该顺手停用账号")
	}
}

/*
「分组状态」只展示显式开启的分组。

用户要的是：「只想展示国内版，国际版不给用户看」。所以默认**不展示** ——
分组建出来是给调度用的，公开状态是额外动作，不能因为建了分组就自动发生。

这个用例同时守住两件容易混在一起的事：
  · 展示开关只影响展示，不会顺手清掉探活模型；
  · 探活模型列表是 `probe_model` 与 `probe_models` 的并集（与调度口径一致）。
*/
func TestChannelStatusVisibilityDefaultsOff(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()

	enabled := true
	create := func(id string) store.Channel {
		t.Helper()
		channel, err := s.CreateChannel(ctx, store.ChannelInput{
			ID:      id,
			Name:    id,
			Adapter: "workbuddy",
			Enabled: &enabled,
			Options: []byte(`{"realm":"intl"}`),
		})
		if err != nil {
			t.Fatalf("新建分组失败: %v", err)
		}
		return *channel
	}

	// 1) 新建即隐藏。
	if channel := create("wb-intl-hidden"); channel.StatusVisible {
		t.Error("新建分组默认不该出现在「分组状态」页")
	}

	// 2) 开启展示 + 设置探活模型。
	visible := true
	probeModels := []string{"glm-4.6", "deepseek-v4.1-flash"}
	probe := ""
	updated, err := s.UpdateChannel(ctx, "wb-intl-hidden", store.ChannelInput{
		StatusVisible: &visible,
		ProbeModels:   &probeModels,
		ProbeModel:    &probe,
	})
	if err != nil {
		t.Fatalf("更新分组失败: %v", err)
	}
	if !updated.StatusVisible {
		t.Error("开启后应标记为展示")
	}
	if len(updated.ProbeModels) != len(probeModels) {
		t.Fatalf("探活模型应为 %v，得到 %v", probeModels, updated.ProbeModels)
	}

	// 3) 关键：关掉展示**不清空**探活模型。
	hidden := false
	closed, err := s.UpdateChannel(ctx, "wb-intl-hidden", store.ChannelInput{StatusVisible: &hidden})
	if err != nil {
		t.Fatalf("关闭展示失败: %v", err)
	}
	if closed.StatusVisible {
		t.Error("关闭后应标记为不展示")
	}
	if len(closed.ProbeModels) != len(probeModels) {
		t.Errorf("关闭展示不该清空探活模型，期望 %v，得到 %v", probeModels, closed.ProbeModels)
	}

	// 4) 状态页只列出开启展示的分组。
	create("wb-cn-shown")
	shown, err := s.UpdateChannel(ctx, "wb-cn-shown", store.ChannelInput{StatusVisible: &visible})
	if err != nil {
		t.Fatalf("开启展示失败: %v", err)
	}
	if !shown.StatusVisible {
		t.Fatal("国内版应已开启展示")
	}

	health, err := s.ListChannelHealth(ctx, time.Time{})
	if err != nil {
		t.Fatalf("读取分组健康度失败: %v", err)
	}
	ids := make(map[string]bool, len(health))
	for _, item := range health {
		ids[item.ID] = true
	}
	if !ids["wb-cn-shown"] {
		t.Error("开启展示的分组应出现在分组状态页")
	}
	if ids["wb-intl-hidden"] {
		t.Error("关闭展示的分组不该出现在分组状态页")
	}
}
