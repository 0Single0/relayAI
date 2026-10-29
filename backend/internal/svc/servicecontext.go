package svc

import (
	"time"

	"relayai/backend/internal/config"
	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"

	"gorm.io/gorm"
)

// ServiceContext 持有服务级依赖，由 handler 层共享。
type ServiceContext struct {
	Config config.Config

	// DB 是 GORM 句柄。迁移与少数需要直连的查询用它。
	DB *gorm.DB

	// Store 是数据访问层（渠道、账号、密钥、用量、任务、设置）。
	Store *store.Store

	// Gateway 是 Rust 网关的内部接口客户端。
	Gateway *gateway.Client

	// startedAt 进程启动时刻，用于 /health 的运行时长。
	startedAt time.Time
}

func NewServiceContext(c config.Config, db *gorm.DB) *ServiceContext {
	return &ServiceContext{
		Config:    c,
		DB:        db,
		Store:     store.New(db),
		Gateway:   gateway.New(c.Gateway),
		startedAt: time.Now(),
	}
}

// UptimeMs 返回进程已运行的毫秒数。
func (s *ServiceContext) UptimeMs() int64 {
	return time.Since(s.startedAt).Milliseconds()
}
