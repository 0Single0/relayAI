package database_test

import (
	"context"
	"os"
	"testing"

	"relayai/backend/internal/database"
)

// TestAutoMigrateFreshSchema 在**全新空库**上跑 AutoMigrate，验证能一次建全。
//
// 需要 `TEST_DATABASE_URL_MIGRATE` 指向一个可以随意清空的库。
//
// 为什么用**独立变量**而不是和 store 测试共用一个库：
//   `go test ./...` 会并发跑各包的测试，而两边都会 `DROP SCHEMA public CASCADE`。
//   共用一个库时会互相清掉对方刚建好的表，表现为随机失败 ——
//   那是最难查的一类 flake。
func TestAutoMigrateFreshSchema(t *testing.T) {
	dsn := os.Getenv("TEST_DATABASE_URL_MIGRATE")
	if dsn == "" {
		t.Skip("未设置 TEST_DATABASE_URL_MIGRATE，跳过建表测试")
	}

	ctx := context.Background()
	db, err := database.Connect(ctx, dsn, database.DefaultOptions())
	if err != nil {
		t.Fatalf("连接测试库失败: %v", err)
	}

	// 清空 public schema，模拟全新库。
	if err := db.Exec("DROP SCHEMA public CASCADE").Error; err != nil {
		t.Fatalf("清空 schema 失败: %v", err)
	}
	if err := db.Exec("CREATE SCHEMA public").Error; err != nil {
		t.Fatalf("重建 schema 失败: %v", err)
	}

	if err := database.Migrate(ctx, db); err != nil {
		t.Fatalf("迁移失败: %v", err)
	}

	// 每张表都应存在。
	for _, name := range []string{
		"channels", "users", "models", "proxies", "api_keys", "accounts",
		"redeem_codes", "settings", "channel_caps", "affinity_bindings",
		"user_sessions", "balance_transactions", "usage_logs", "usage_payloads",
		"task_runs", "task_run_items",
	} {
		if !db.Migrator().HasTable(name) {
			t.Errorf("表 %s 未创建", name)
		}
	}

	// 幂等：再跑一次不应报错，也不应改动任何东西。
	if err := database.Migrate(ctx, db); err != nil {
		t.Fatalf("二次迁移失败（应幂等）: %v", err)
	}
}
