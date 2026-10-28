package database

import (
	"context"
	"embed"
	"fmt"
	"io/fs"
	"sort"
	"strings"

	"github.com/zeromicro/go-zero/core/logx"
)

//go:embed all:migrations/*.sql
var migrationFS embed.FS

// migrationTable 记录已执行的迁移版本。
const migrationTable = `
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);`

// Migrate 按文件名顺序执行尚未应用的迁移。
//
// 每个迁移在自己的事务里执行：失败则整个文件回滚，不会留下半截表结构。
// 已应用的版本记录在 schema_migrations 表，重复调用是幂等的。
func Migrate(ctx context.Context, pool *Pool) error {
	if _, err := pool.Exec(ctx, migrationTable); err != nil {
		return fmt.Errorf("创建迁移记录表失败: %w", err)
	}

	applied, err := appliedVersions(ctx, pool)
	if err != nil {
		return err
	}

	versions, err := listMigrations()
	if err != nil {
		return err
	}

	for _, version := range versions {
		if _, ok := applied[version]; ok {
			continue
		}

		body, err := migrationFS.ReadFile("migrations/" + version)
		if err != nil {
			return fmt.Errorf("读取迁移 %s 失败: %w", version, err)
		}

		if err := applyOne(ctx, pool, version, string(body)); err != nil {
			return err
		}

		logx.Infof("迁移已应用: %s", version)
	}

	return nil
}

// applyOne 在单个事务里执行一份迁移并记录版本。
func applyOne(ctx context.Context, pool *Pool, version, body string) error {
	tx, err := pool.Begin(ctx)
	if err != nil {
		return fmt.Errorf("开启事务失败（迁移 %s）: %w", version, err)
	}
	defer func() { _ = tx.Rollback(ctx) }()

	// pgx 的 Exec 支持多语句（simple protocol），迁移文件里可以写多段 DDL。
	if _, err := tx.Exec(ctx, body); err != nil {
		return fmt.Errorf("执行迁移 %s 失败: %w", version, err)
	}

	if _, err := tx.Exec(ctx, "INSERT INTO schema_migrations (version) VALUES ($1)", version); err != nil {
		return fmt.Errorf("记录迁移版本失败（%s）: %w", version, err)
	}

	if err := tx.Commit(ctx); err != nil {
		return fmt.Errorf("提交迁移失败（%s）: %w", version, err)
	}

	return nil
}

// appliedVersions 读取已应用的迁移版本集合。
func appliedVersions(ctx context.Context, pool *Pool) (map[string]struct{}, error) {
	rows, err := pool.Query(ctx, "SELECT version FROM schema_migrations")
	if err != nil {
		return nil, fmt.Errorf("读取迁移记录失败: %w", err)
	}
	defer rows.Close()

	applied := make(map[string]struct{})
	for rows.Next() {
		var version string
		if err := rows.Scan(&version); err != nil {
			return nil, fmt.Errorf("扫描迁移记录失败: %w", err)
		}
		applied[version] = struct{}{}
	}

	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("遍历迁移记录失败: %w", err)
	}

	return applied, nil
}

// listMigrations 返回按文件名排序的迁移文件名（只取 .sql）。
func listMigrations() ([]string, error) {
	entries, err := fs.ReadDir(migrationFS, "migrations")
	if err != nil {
		return nil, fmt.Errorf("读取迁移目录失败: %w", err)
	}

	versions := make([]string, 0, len(entries))
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasSuffix(name, ".sql") {
			continue
		}
		versions = append(versions, name)
	}

	sort.Strings(versions)
	return versions, nil
}
