// Package database 提供 PostgreSQL 连接池与迁移执行器。
//
// 为什么自己写迁移运行器而不用 goose / migrate：
//   1. 本项目只有单向迁移（不做 down），需求简单；
//   2. 迁移文件用 embed 打进二进制，部署时不需要额外带目录；
//   3. 多一个依赖就多一份供应链风险，而这里的逻辑不到 100 行。
package database

import (
	"context"
	"fmt"
	"time"

	"github.com/jackc/pgx/v5/pgxpool"
)

// Pool 是全局连接池的别名，避免调用方到处 import pgxpool。
type Pool = pgxpool.Pool

// Options 是连接池的可调参数。
//
// 默认值面向单机自用场景：并发不高，但流式请求会长时间占用连接，
// 所以 MaxConns 给得比默认（4）宽一些。
type Options struct {
	// MaxConns 池内最大连接数。
	MaxConns int32
	// MinConns 预热连接数。
	MinConns int32
	// MaxConnLifetime 连接最长寿命。
	MaxConnLifetime time.Duration
	// MaxConnIdleTime 空闲多久回收。
	MaxConnIdleTime time.Duration
	// ConnectTimeout 建立连接的超时。
	ConnectTimeout time.Duration
}

// DefaultOptions 返回自用场景的默认连接池参数。
func DefaultOptions() Options {
	return Options{
		MaxConns:        16,
		MinConns:        2,
		MaxConnLifetime: time.Hour,
		MaxConnIdleTime: 15 * time.Minute,
		ConnectTimeout:  10 * time.Second,
	}
}

// Open 建立连接池并做一次 Ping，确保配置真的可用。
//
// 失败时返回带上下文的错误，而不是让调用方在第一次查询时才发现连不上。
func Open(ctx context.Context, dataSource string, opts Options) (*Pool, error) {
	if dataSource == "" {
		return nil, fmt.Errorf("数据库连接串为空：请设置 DATABASE_URL 或配置文件里的 DataSource")
	}

	cfg, err := pgxpool.ParseConfig(dataSource)
	if err != nil {
		return nil, fmt.Errorf("解析数据库连接串失败: %w", err)
	}

	if opts.MaxConns > 0 {
		cfg.MaxConns = opts.MaxConns
	}
	if opts.MinConns >= 0 {
		cfg.MinConns = opts.MinConns
	}
	if opts.MaxConnLifetime > 0 {
		cfg.MaxConnLifetime = opts.MaxConnLifetime
	}
	if opts.MaxConnIdleTime > 0 {
		cfg.MaxConnIdleTime = opts.MaxConnIdleTime
	}
	if opts.ConnectTimeout > 0 {
		cfg.ConnConfig.ConnectTimeout = opts.ConnectTimeout
	}

	pool, err := pgxpool.NewWithConfig(ctx, cfg)
	if err != nil {
		return nil, fmt.Errorf("创建连接池失败: %w", err)
	}

	pingCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	if err := pool.Ping(pingCtx); err != nil {
		pool.Close()
		return nil, fmt.Errorf("连接数据库失败: %w", err)
	}

	return pool, nil
}
