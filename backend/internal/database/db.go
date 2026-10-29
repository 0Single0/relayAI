// Package database 提供 GORM 连接、迁移与 schema 演进。
//
// 迁移机制已从「手写 SQL 文件 + 自研执行器」换成 GORM 的 `Migrator()`：
// 表结构由 `internal/models` 里的结构体定义，AutoMigrate 负责建表与补齐差额。
// 破坏性变更（删列/改名/数据搬迁）走 `explicitMigrations`，显式且留痕。
package database

import (
	"context"
	"fmt"
	"time"

	"gorm.io/gorm"
)

// Options 是连接池的可调参数。
//
// 默认值面向单机自用场景：并发不高，但流式请求会长时间占用连接，
// 所以 MaxOpenConns 给得比 GORM 默认（很小的匿名池）宽一些。
type Options struct {
	// MaxOpenConns 池内最大连接数。
	MaxOpenConns int
	// MaxIdleConns 常驻空闲连接数。
	MaxIdleConns int
	// MaxLifetime 连接最长寿命。
	MaxLifetime time.Duration
	// MaxIdleTime 空闲多久回收。
	MaxIdleTime time.Duration
}

// DefaultOptions 返回自用场景的默认连接池参数。
func DefaultOptions() Options {
	return Options{
		MaxOpenConns: 20,
		MaxIdleConns: 5,
		MaxLifetime:  time.Hour,
		MaxIdleTime:  15 * time.Minute,
	}
}

// Connect 建立 GORM 连接并做一次 Ping。
//
// 失败时返回带上下文的错误，而不是让调用方在第一次查询时才发现连不上。
func Connect(ctx context.Context, dataSource string, opts Options) (*gorm.DB, error) {
	if dataSource == "" {
		return nil, fmt.Errorf("数据库连接串为空：请设置 DATABASE_URL 或配置文件里的 DataSource")
	}

	db, err := Open(dataSource, false)
	if err != nil {
		return nil, err
	}

	sqlDB, err := db.DB()
	if err != nil {
		return nil, fmt.Errorf("获取底层连接失败: %w", err)
	}
	if opts.MaxOpenConns > 0 {
		sqlDB.SetMaxOpenConns(opts.MaxOpenConns)
	}
	if opts.MaxIdleConns >= 0 {
		sqlDB.SetMaxIdleConns(opts.MaxIdleConns)
	}
	if opts.MaxLifetime > 0 {
		sqlDB.SetConnMaxLifetime(opts.MaxLifetime)
	}
	if opts.MaxIdleTime > 0 {
		sqlDB.SetConnMaxIdleTime(opts.MaxIdleTime)
	}

	pingCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	if err := sqlDB.PingContext(pingCtx); err != nil {
		_ = sqlDB.Close()
		return nil, fmt.Errorf("连接数据库失败: %w", err)
	}

	return db, nil
}
