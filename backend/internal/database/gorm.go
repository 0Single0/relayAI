// Package database 负责连接池、迁移与 schema 版本管理。
package database

import (
	"fmt"

	"relayai/backend/internal/models"

	"gorm.io/driver/postgres"
	"gorm.io/gorm"
	"gorm.io/gorm/logger"
)

// All 返回全部模型，**顺序即建表顺序**。
//
// 为什么手工维护顺序而不是靠反射扫描：
//   · 反射会拿到不确定的顺序，外键（`usage_logs → channels/users`）可能先建子表；
//   · 显式列表在 review 时能一眼看出"新表加在哪、依赖谁"。
//
// 加新表时**必须**同时加进这里，否则 AutoMigrate 不会创建它。
func All() []any {
	return []any{
		// 被引用的主表在前。
		&models.Channels{},
		&models.Users{},
		&models.Models{},
		&models.Proxies{},
		&models.APIKeys{},
		&models.Accounts{},
		&models.RedeemCodes{},
		&models.Settings{},
		&models.ChannelCaps{},
		&models.AffinityBindings{},
		&models.UserSessions{},
		&models.BalanceTransactions{},
		&models.UsageLogs{},
		&models.UsagePayloads{},
		&models.TaskRuns{},
		&models.TaskRunItems{},
		&models.ChannelProbes{},
	}
}

// Open 建立 GORM 连接。
//
// `PrepareStmt` 打开语句缓存：本项目的查询绝大多数是参数化短查询，
// 缓存预编译语句能省下每次解析的开销（与原先 pgx 的行为接近）。
func Open(dsn string, debug bool) (*gorm.DB, error) {
	logLevel := logger.Warn
	if debug {
		logLevel = logger.Info
	}
	db, err := gorm.Open(postgres.Open(dsn), &gorm.Config{
		Logger:                                   logger.Default.LogMode(logLevel),
		PrepareStmt:                              true,
		DisableForeignKeyConstraintWhenMigrating: false,
	})
	if err != nil {
		return nil, fmt.Errorf("连接数据库失败: %w", err)
	}

	sqlDB, err := db.DB()
	if err != nil {
		return nil, fmt.Errorf("获取底层连接失败: %w", err)
	}
	// 面向单机自用：并发不高，但流式请求会长时间占连接，所以比默认值宽。
	sqlDB.SetMaxOpenConns(20)
	sqlDB.SetMaxIdleConns(5)
	return db, nil
}
