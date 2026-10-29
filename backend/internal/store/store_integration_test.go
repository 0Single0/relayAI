package store_test

import (
	"context"
	"os"
	"testing"

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
