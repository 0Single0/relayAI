package config

import "github.com/zeromicro/go-zero/rest"

// Config 是后端服务的完整配置。
//
// 数据库连接串优先从环境变量 DATABASE_URL 读取（见 backend.go），
// 配置文件里的 DataSource 作为本地开发的兜底。
type Config struct {
	rest.RestConf

	// Database 是 PostgreSQL 相关配置。
	Database DatabaseConf

	// Gateway 是 Rust 网关的内部接口地址，管理面通过它代理必须贴适配器的操作。
	Gateway GatewayConf
}

// DatabaseConf 是数据库配置。
type DatabaseConf struct {
	// DataSource 是 PostgreSQL 连接串。
	// 生产环境用环境变量 DATABASE_URL 覆盖，避免把密码写进配置文件。
	DataSource string `json:",optional"`

	// MaxConns 池内最大连接数，0 表示用默认值。
	MaxConns int `json:",default=16"`

	// MinConns 预热连接数。
	MinConns int `json:",default=2"`

	// AutoMigrate 是否在启动时自动执行迁移。
	// 本地开发开，生产建议关掉、由部署流程显式执行。
	AutoMigrate bool `json:",default=true"`
}

// GatewayConf 是 Rust 网关的内部接口配置。
type GatewayConf struct {
	// BaseURL 网关地址，如 http://127.0.0.1:18788。
	BaseURL string `json:",default=http://127.0.0.1:18788"`

	// PublicBaseURL 是对外公布的网关地址（客户端填 base URL 用）。
	//
	// 留空则回落到 BaseURL —— 本地开发两者相同。
	// 但**部署时必须显式设置**：BaseURL 往往是 127.0.0.1（后端到网关的内网地址），
	// 用户在自己电脑上填 127.0.0.1 打的是他自己的机器。
	PublicBaseURL string `json:",optional"`

	// InternalToken 内部接口共享密钥，必须与网关的 GATEWAY_INTERNAL_TOKEN 一致。
	InternalToken string `json:",optional"`

	// TimeoutMs 调用网关的超时（毫秒）。
	TimeoutMs int `json:",default=15000"`
}
