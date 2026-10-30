// Package models 定义全部数据库表结构。
//
// 为什么集中在一个包：
//   · 表之间有外键与复用关系（`usage_logs` 引用 users / api_keys / channels），
//     分散在各自业务包里会形成循环依赖；
//   · GORM 的 AutoMigrate 需要一次性拿到**全部**模型才能正确排外键顺序。
//
// 约定（全部显式写，不依赖 GORM 的自动推断）：
//   · 表名：`TableName()`，与既有 SQL 迁移建出的表名完全一致；
//   · 列名：`gorm:"column:xxx"`，现有列是 snake_case，
//     自动转换在 `channel_ratio` / `key_hash` 这类词上会出错；
//   · 金额：`*float64` + `type:numeric(14,8)`。**不用 float32** ——
//     曾因 COALESCE 的类型推断把 0.8 截成 0，精度必须显式保住；
//   · 时间：`time.Time` + `type:timestamptz`，与现有 `timestamp with time zone` 对齐；
//   · 索引用显式名字，避免 AutoMigrate 生成 `idx_xxx_yyy` 这种与现有不一致的名字。
package models

import "time"

// Channels 是计价分组。它只管倍率与并发/QPS 上限。
//
// 平台与凭据属于账号（`Accounts.Platform`），一个分组下可以混不同平台的号。
type Channels struct {
	ID       string `gorm:"column:id;primaryKey;type:text" json:"id"`
	Name     string `gorm:"column:name;type:text;not null" json:"name"`
	Adapter  string `gorm:"column:adapter;type:text;not null" json:"adapter"`
	Enabled  bool   `gorm:"column:enabled;not null;default:true" json:"enabled"`
	/*
		分组级 priority / weight 保留列但**不参与调度**。

		Key 与分组是 1:1 强绑定（handler 层强制 ChannelAllow 长度为 1），
		所以候选集进入路由前就只剩一个分组，这两个值永远不可能被比较。
		保留是为了兼容既有数据，新代码不该依赖它们。
	*/
	Priority  int   `gorm:"column:priority;type:integer;not null;default:100" json:"priority"`
	Weight    int   `gorm:"column:weight;type:integer;not null;default:1" json:"weight"`
	Options   JSONB `gorm:"column:options;type:jsonb;not null;default:'{}'" json:"options"`
	ChRatio   float64 `gorm:"column:channel_ratio;type:numeric(14,8);not null;default:1" json:"channelRatio"`
	ConcurrencyLimit *int `gorm:"column:concurrency_limit;type:integer" json:"concurrencyLimit"`
	RateLimitQPS     *int `gorm:"column:rate_limit_qps;type:integer" json:"rateLimitQps"`
	// ModelAllow 为 nil 表示不限制；非 nil 时只暴露这些模型。
	ModelAllow TextArray `gorm:"column:model_allow;type:text[]" json:"modelAllow"`
	// ProbeModel 是**旧版单选**探活模型，保留兼容。
	//
	// 新代码用 ProbeModels（多选）。留着它不是舍不得删，
	// 而是老数据里这个字段已经填了值 —— 迁移时把它并进 ProbeModels，
	// 然后这里只作为回读兼容。
	ProbeModel string `gorm:"column:probe_model;type:text" json:"probeModel"`
	// ProbeModels 是本分组要探活的**模型列表**。
	//
	// 为什么是列表：一个分组挂十几个模型，只探一个得不出"分组是否可用"。
	// 空表示"探这个分组对外暴露的全部模型"。
	ProbeModels TextArray `gorm:"column:probe_models;type:text[]" json:"probeModels"`
	// StatusVisible 决定这个分组是否出现在「分组状态」页。
	//
	// **默认 false（不展示）**：那张页面是给人看的门面，
	// 内部用来调度的分组不该自动出现在上面。要展示哪个分组是明确的选择，
	// 不是"建了分组就自动公开"。
	//
	// 只影响展示，不影响探活：探活由 `probe_model` / `probe_models` 决定，
	// 两者互不牵连 —— 否则"临时把某个分组藏起来"会顺带清空它的探活历史。
	StatusVisible bool      `gorm:"column:status_visible;not null;default:false" json:"statusVisible"`
	CreatedAt     time.Time `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
	UpdatedAt  time.Time `gorm:"column:updated_at;not null;default:now()" json:"updatedAt"`
}

func (Channels) TableName() string { return "channels" }

// Accounts 是上游账号。凭据加密后存 `credential_cipher`。
type Accounts struct {
	ID int64 `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	/*
		ChannelID 可空：账号可以先建、之后再分配分组。

		为什么允许空：建号时（尤其批量导入、授权刚完成）常常还没想好归到哪个计价桶。
		强制先选分组会导致"为了建号先随便建一个分组"，那比留空更脏。

		未分配分组的账号**不参与调度** —— 网关按 channel 取账号，空分组下没有号可发。
		界面上单独标识为"待分配"。

		唯一性改由 `accounts_uid_key`（单列 uid）保证：
		`(channel_id, uid)` 在 channel_id 为 NULL 时不再有约束力
		（Postgres 的 NULL 不等于 NULL）。
	*/
	ChannelID *string `gorm:"column:channel_id;type:text;index:idx_accounts_channel" json:"channelId"`
	// UID 是上游账号标识。
	//
	// 单列唯一而不是 (channel_id, uid)：一个上游账号只该在库里出现一次，
	// 换分组是迁移不是新建。这样"先建号后分配"也不会因为换组而插入重复行。
	UID        string `gorm:"column:uid;type:text;not null;uniqueIndex:accounts_uid_key" json:"uid"`
	Nickname   string `gorm:"column:nickname;type:text;not null;default:''" json:"nickname"`
	Enabled    bool   `gorm:"column:enabled;not null;default:true" json:"enabled"`
	// CredentialCipher 是加密后的凭据。明文永不落库、永不进日志。
	CredentialCipher []byte `gorm:"column:credential_cipher;type:bytea" json:"-"`
	/*
		账号级 priority / weight 是**真正生效**的调度参数：
		同一分组下多个号之间，priority 分层、weight 分摊容量。
	*/
	PriorityOverride *int      `gorm:"column:priority_override;type:integer" json:"priorityOverride"`
	WeightOverride   *int      `gorm:"column:weight_override;type:integer" json:"weightOverride"`
	MaxInflight      *int      `gorm:"column:max_inflight;type:integer" json:"maxInflight"`
	ModelAllow       TextArray `gorm:"column:model_allow;type:text[]" json:"modelAllow"`
	ModelRename      JSONB     `gorm:"column:model_rename;type:jsonb;not null;default:'{}'" json:"modelRename"`
	Source           string    `gorm:"column:source;type:text;not null;default:'manual'" json:"source"`
	BaseURL          string    `gorm:"column:base_url;type:text;not null;default:''" json:"baseUrl"`
	Protocol         string    `gorm:"column:protocol;type:text;not null;default:'openai-chat'" json:"protocol"`
	Platform         string    `gorm:"column:platform;type:text;not null;default:'custom'" json:"platform"`
	Options          JSONB     `gorm:"column:options;type:jsonb;not null;default:'{}'" json:"options"`
	ProxyID          *int64    `gorm:"column:proxy_id" json:"proxyId"`
	AddedAt          time.Time `gorm:"column:added_at;not null;default:now()" json:"addedAt"`
}

func (Accounts) TableName() string { return "accounts" }

// Users 是管理台用户。
type Users struct {
	ID           int64  `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	Email        string `gorm:"column:email;type:text;not null;index:users_email_idx" json:"email"`
	PasswordHash string `gorm:"column:password_hash;type:text;not null" json:"-"`
	Nickname     string `gorm:"column:nickname;type:text;not null;default:''" json:"nickname"`
	/*
		金额用 numeric(14,8) + float64。

		为什么不存整数分：现有数据与计费链路全部按"元"的小数计算，
		中途换单位会让历史账单与实收对不上。
	*/
	Balance        float64 `gorm:"column:balance;type:numeric(14,8);not null;default:0" json:"balance"`
	TotalRecharged float64 `gorm:"column:total_recharged;type:numeric(14,8);not null;default:0" json:"totalRecharged"`
	Role           string  `gorm:"column:role;type:text;not null;default:'user'" json:"role"`
	Enabled        bool    `gorm:"column:enabled;not null;default:true" json:"enabled"`
	Source         string  `gorm:"column:source;type:text;not null;default:'password'" json:"source"`
	CreatedAt      time.Time  `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
	LastLoginAt    *time.Time `gorm:"column:last_login_at" json:"lastLoginAt"`
}

func (Users) TableName() string { return "users" }

// APIKeys 是下发给客户端的密钥。
//
// Key 与分组 1:1 强制绑定：`ChannelAllow` 长度必须恰好为 1（handler 层校验）。
// 允许留空等于允许它走任意通道，那样"便宜的 Key / 稳定的 Key"分流就不成立。
type APIKeys struct {
	ID          int64  `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	Name        string `gorm:"column:name;type:text;not null" json:"name"`
	KeyHash     string `gorm:"column:key_hash;type:text;not null;uniqueIndex:idx_api_keys_hash" json:"-"`
	KeyPrefix   string `gorm:"column:key_prefix;type:text;not null" json:"keyPrefix"`
	ChannelAllow TextArray `gorm:"column:channel_allow;type:text[]" json:"channelAllow"`
	ModelAllow   TextArray `gorm:"column:model_allow;type:text[]" json:"modelAllow"`
	RPMLimit         *int   `gorm:"column:rpm_limit;type:integer" json:"rpmLimit"`
	DailyTokenLimit  *int64 `gorm:"column:daily_token_limit" json:"dailyTokenLimit"`
	MaxConcurrency   *int   `gorm:"column:max_concurrency;type:integer" json:"maxConcurrency"`
	Enabled          bool   `gorm:"column:enabled;not null;default:true" json:"enabled"`
	ExpiresAt        *time.Time `gorm:"column:expires_at" json:"expiresAt"`
	LastUsedAt       *time.Time `gorm:"column:last_used_at" json:"lastUsedAt"`
	CreatedAt        time.Time  `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
	UserID           *int64     `gorm:"column:user_id" json:"userId"`
	// KeyPlaintext 是用户主动勾选"可见"时才存的明文，其余情况为 NULL。
	KeyPlaintext *string `gorm:"column:key_plaintext;type:text" json:"keyPlaintext"`
}

func (APIKeys) TableName() string { return "api_keys" }

// Models 是模型目录与定价。
type Models struct {
	CanonicalName  string    `gorm:"column:canonical_name;primaryKey;type:text" json:"name"`
	DisplayName    string    `gorm:"column:display_name;type:text;not null;default:''" json:"displayName"`
	ContextWindow  *int64    `gorm:"column:context_window;type:integer" json:"contextWindow"`
	MaxOutput      *int64    `gorm:"column:max_output;type:integer" json:"maxOutput"`
	Capabilities   JSONB     `gorm:"column:capabilities;type:jsonb;not null;default:'{}'" json:"capabilities"`
	InputPrice     float64   `gorm:"column:input_price;type:numeric(14,8);not null;default:0" json:"inputPrice"`
	OutputPrice    float64   `gorm:"column:output_price;type:numeric(14,8);not null;default:0" json:"outputPrice"`
	CacheReadPrice float64   `gorm:"column:cache_read_price;type:numeric(14,8);not null;default:0" json:"cacheReadPrice"`
	ModelRatio     float64   `gorm:"column:model_ratio;type:numeric(14,8);not null;default:1" json:"modelRatio"`
	Enabled        bool      `gorm:"column:enabled;not null;default:true" json:"enabled"`
}

func (Models) TableName() string { return "models" }
