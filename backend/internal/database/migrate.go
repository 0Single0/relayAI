package database

import (
	"context"
	"fmt"
	"strings"

	"github.com/zeromicro/go-zero/core/logx"
	"gorm.io/gorm"

	"relayai/backend/internal/models"
)

/*
迁移机制换成 GORM 的 `Migrator()`。

与旧实现（18 个手写 SQL 文件 + 自己的 schema_migrations 执行器）的区别：

	旧：文件名排序 → 逐个文件执行 SQL → 记录版本号。
	新：`AutoMigrate` 对比模型定义与实时 schema，**只补齐差额**。
	    加列、加索引、放宽约束是幂等的；删列/改名它不会做。

为什么 `AutoMigrate` 够用：
  - 它按"结构体 → 表"单向推导，重复执行不产生副作用；
  - 版本号不再需要：结构体的当前形态就是唯一事实，
    不存在"哪个 SQL 文件跑到第几个"这种状态。

什么时候必须写显式迁移（见 `explicitMigrations`）：
  - **删列 / 改名**：AutoMigrate 故意不处理，因为那是破坏性操作；
  - **数据搬迁**：把某列的值算进另一列、回填默认值；
  - **无法用 tag 表达的约束**：CHECK、部分索引、复杂唯一约束。
*/

// migrationEntry 是一条显式迁移。
type migrationEntry struct {
	// version 是稳定标识，一旦发布就不要改。
	version string
	// up 是具体的变更。返回错误则整条迁移回滚。
	up func(tx *gorm.DB) error
}

// explicitMigrations 是**按需追加**的表结构变更。
//
// 当前为空：所有表都由 `All()` 里的模型定义描述，AutoMigrate 一次建全。
// 后续遇到"删列 / 改名 / 数据搬迁"再往这里加。
var explicitMigrations = []migrationEntry{
	/*
		账号可以先建、后分配分组（`channel_id` 允许 NULL）。

		为什么必须显式做而不能只改 struct tag：
		  · AutoMigrate **不会**把 `NOT NULL` 放宽成可空 —— 那是放宽约束，
		    它只做加列/加索引这类安全操作；
		  · 旧的 `(channel_id, uid)` 唯一约束在 channel_id 为 NULL 时失效
		    （Postgres 里 NULL <> NULL），要换成 `uid` 单列唯一。

		顺序要紧：先删旧约束，再放宽 null。反过来会在中间态出现
		"同一个 uid 能插两次"的窗口。
	*/
	{
		version: "20261029_account_channel_nullable",
		up: func(tx *gorm.DB) error {
			if tx.Migrator().HasConstraint(&models.Accounts{}, "accounts_channel_uid_key") {
				if err := tx.Migrator().DropConstraint(&models.Accounts{}, "accounts_channel_uid_key"); err != nil {
					return err
				}
			}
			// 外键不受影响：NULL 不违反外键。
			return tx.Exec("ALTER TABLE accounts ALTER COLUMN channel_id DROP NOT NULL").Error
		},
	},
}

// Migrate 执行迁移：先 AutoMigrate 建/补表结构，再跑未执行的显式迁移。
func Migrate(ctx context.Context, db *gorm.DB) error {
	if err := ensureMigrationTable(db); err != nil {
		return err
	}

	all := All()
	logx.Infof("开始迁移：AutoMigrate %d 张表", len(all))

	/*
	  逐表迁移，**单表失败不中断整轮**。

	  为什么必须这样：`AutoMigrate(all...)` 是一趟串行的 DDL，
	  其中任何一句报错就整体返回 —— 后面的表根本没建。

	  实测过的后果：老库的 `users_email_key` 与 GORM 期望的 `uni_users_email`
	  不一致，迁移在第一张表就抛 SQLSTATE 42704，
	  于是新加的 `channel_probes` 永远建不出来，而日志只显示"忽略约束差异"，
	  看起来像成功了。

	  逐表跑之后，一张表的约束命名差异只影响它自己。
	*/
	var failures []string
	for _, model := range all {
		if err := db.WithContext(ctx).AutoMigrate(model); err != nil {
			message := err.Error()
			/*
			  已有唯一约束的名字与 GORM 期望不一致时，它会尝试
			  `ALTER TABLE ... DROP CONSTRAINT "uni_xxx"`，而那条约束根本不存在。
			  这是**非致命**的：表结构本身是好的。

			  用 SQLSTATE 42704（undefined_object）判断比匹配文案可靠 ——
			  报错里 `DROP CONSTRAINT` 与 `does not exist` 中间夹着约束名。
			*/
			renameOnly := strings.Contains(message, "SQLSTATE 42704") &&
				strings.Contains(message, "constraint")
			if renameOnly {
				logx.Infof("忽略约束改名差异（%T）：%s", model, message)
				continue
			}
			failures = append(failures, fmt.Sprintf("%T: %s", model, message))
		}
	}
	if len(failures) > 0 {
		return fmt.Errorf("AutoMigrate 有 %d 张表失败: %s", len(failures), strings.Join(failures, "; "))
	}

	applied, err := appliedMigrations(db)
	if err != nil {
		return err
	}

	for _, entry := range explicitMigrations {
		if applied[entry.version] {
			continue
		}
		// 每条显式迁移独立事务：一条失败不影响其它已成功的。
		if err := db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
			if err := entry.up(tx); err != nil {
				return err
			}
			return tx.Exec(
				"INSERT INTO schema_migrations (version, applied_at) VALUES (?, now())",
				entry.version,
			).Error
		}); err != nil {
			return fmt.Errorf("显式迁移 %s 失败: %w", entry.version, err)
		}
		logx.Infof("显式迁移已应用：%s", entry.version)
	}

	return nil
}

// ensureMigrationTable 建版本记录表。
//
// 表名沿用旧实现的 `schema_migrations`：老库里已有这张表和 18 条记录。
// 那些记录代表"SQL 文件跑过"，与新的显式迁移不是一套编号，
// 所以读取时只认 `explicitMigrations` 里的版本，老记录天然被忽略。
func ensureMigrationTable(db *gorm.DB) error {
	const ddl = `
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);`
	if err := db.Exec(ddl).Error; err != nil {
		return fmt.Errorf("创建迁移记录表失败: %w", err)
	}
	return nil
}

func appliedMigrations(db *gorm.DB) (map[string]bool, error) {
	var versions []string
	if err := db.Raw("SELECT version FROM schema_migrations").Scan(&versions).Error; err != nil {
		return nil, fmt.Errorf("读取迁移记录失败: %w", err)
	}
	out := make(map[string]bool, len(versions))
	for _, version := range versions {
		out[version] = true
	}
	return out, nil
}

// AutoMigrateOnly 只建表、不跑显式迁移。给测试用。
func AutoMigrateOnly(db *gorm.DB) error {
	return db.AutoMigrate(All()...)
}
