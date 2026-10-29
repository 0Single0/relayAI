package models

import "time"

// UsageLogs 是请求级账单，**只追加不修改**。
//
// 一张表同时承担两种用途，靠字段区分：
//   - 请求事实：谁、什么模型、多少 token、多快；
//   - 计费快照：当时的单价与倍率（input_price…channel_ratio）。
//
// 快照字段必须冗余存：单价会变，不存当时的值，历史账单就对不上。
type UsageLogs struct {
	ID        int64     `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	TS        time.Time `gorm:"column:ts;not null;default:now();index:idx_usage_ts" json:"ts"`
	RequestID string    `gorm:"column:request_id;type:text;not null" json:"requestId"`

	KeyID      *int64 `gorm:"column:key_id;index:idx_usage_key" json:"keyId"`
	UserID     *int64 `gorm:"column:user_id;index:idx_usage_user" json:"userId"`
	ChannelID  string `gorm:"column:channel_id;type:text;not null" json:"channelId"`
	AccountUID string `gorm:"column:account_uid;type:text;not null" json:"accountUid"`

	Protocol      string `gorm:"column:protocol;type:text;not null" json:"protocol"`
	Model         string `gorm:"column:model;type:text;not null;index:idx_usage_model" json:"model"`
	UpstreamModel *string `gorm:"column:upstream_model;type:text" json:"upstreamModel"`

	Stream     bool   `gorm:"column:stream;not null;default:false" json:"stream"`
	Status     string `gorm:"column:status;type:text;not null" json:"status"`
	IsRejection bool  `gorm:"column:is_rejection;not null;default:false" json:"isRejection"`
	Attempts   int    `gorm:"column:attempts;type:integer;not null;default:1" json:"attempts"`

	PromptTokens     int64 `gorm:"column:prompt_tokens;not null;default:0" json:"promptTokens"`
	CompletionTokens int64 `gorm:"column:completion_tokens;not null;default:0" json:"completionTokens"`
	ReasoningTokens  int64 `gorm:"column:reasoning_tokens;not null;default:0" json:"reasoningTokens"`
	CachedTokens     int64 `gorm:"column:cached_tokens;not null;default:0" json:"cachedTokens"`
	TotalTokens      int64 `gorm:"column:total_tokens;not null;default:0" json:"totalTokens"`
	// UsageEstimated 表示 token 数来自估算而非上游回传。
	UsageEstimated bool `gorm:"column:usage_estimated;not null;default:false" json:"usageEstimated"`

	Credit *float64 `gorm:"column:credit;type:numeric(14,8)" json:"credit"`
	/*
		upstream_cost 与 billed_cost **必须分开**：
		前者是上游真实花了多少，后者是向用户收了多少。
		合成一个字段就无法回答"这个分组到底赚不赚"。
	*/
	UpstreamCost float64 `gorm:"column:upstream_cost;type:numeric(14,8);not null;default:0" json:"upstreamCost"`
	BilledCost   float64 `gorm:"column:billed_cost;type:numeric(14,8);not null;default:0" json:"billedCost"`

	TTFTMS  *int  `gorm:"column:ttft_ms;type:integer" json:"ttftMs"`
	TotalMS int64 `gorm:"column:total_ms;type:integer;not null;default:0" json:"totalMs"`
	Effort  *string `gorm:"column:effort;type:text" json:"effort"`
	ErrorDetail *string `gorm:"column:error_detail;type:text" json:"errorDetail"`

	// SettledAt 非空表示已计入用户余额。结算幂等的依据。
	SettledAt *time.Time `gorm:"column:settled_at;index:idx_usage_unsettled" json:"settledAt"`

	// 价格快照。
	InputPrice     *float64 `gorm:"column:input_price;type:numeric(14,8)" json:"inputPrice"`
	OutputPrice    *float64 `gorm:"column:output_price;type:numeric(14,8)" json:"outputPrice"`
	CacheReadPrice *float64 `gorm:"column:cache_read_price;type:numeric(14,8)" json:"cacheReadPrice"`
	ModelRatio     *float64 `gorm:"column:model_ratio;type:numeric(14,8)" json:"modelRatio"`
	ChannelRatio   *float64 `gorm:"column:channel_ratio;type:numeric(14,8)" json:"channelRatio"`
}

func (UsageLogs) TableName() string { return "usage_logs" }

// UsagePayloads 保存请求/响应原文，按保留策略定期清理。
type UsagePayloads struct {
	UsageID  int64     `gorm:"column:usage_id;primaryKey" json:"usageId"`
	Request  JSONB     `gorm:"column:request;type:jsonb;not null;default:'{}'" json:"request"`
	Response JSONB     `gorm:"column:response;type:jsonb;not null;default:'{}'" json:"response"`
	Bytes    int       `gorm:"column:bytes;type:integer;not null;default:0" json:"bytes"`
	CreatedAt time.Time `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
}

func (UsagePayloads) TableName() string { return "usage_payloads" }

// TaskRuns 是一次批量执行。
type TaskRuns struct {
	ID         int64      `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	BatchID    *string    `gorm:"column:batch_id;type:uuid" json:"batchId"`
	ChannelID  string     `gorm:"column:channel_id;type:text;not null;index:idx_task_runs_channel" json:"channelId"`
	AccountUID string     `gorm:"column:account_uid;type:text;not null" json:"accountUid"`
	Scope      string     `gorm:"column:scope;type:text;not null" json:"scope"`
	Status     string     `gorm:"column:status;type:text;not null" json:"status"`
	// Trigger 区分手动与定时：`manual` / `schedule`。
	Trigger      string `gorm:"column:trigger;type:text;not null" json:"trigger"`
	Total        int    `gorm:"column:total;type:integer;not null;default:0" json:"total"`
	DoneCount    int    `gorm:"column:done_count;type:integer;not null;default:0" json:"doneCount"`
	FailedCount  int    `gorm:"column:failed_count;type:integer;not null;default:0" json:"failedCount"`
	SkippedCount int    `gorm:"column:skipped_count;type:integer;not null;default:0" json:"skippedCount"`
	Earned       int    `gorm:"column:earned;type:integer;not null;default:0" json:"earned"`
	Error        *string `gorm:"column:error;type:text" json:"error"`
	StartedAt    time.Time `gorm:"column:started_at;not null;default:now()" json:"startedAt"`
	FinishedAt   *time.Time `gorm:"column:finished_at" json:"finishedAt"`
}

func (TaskRuns) TableName() string { return "task_runs" }

// TaskRunItems 是一次执行里的单条明细。
type TaskRunItems struct {
	ID     int64  `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	RunID  int64  `gorm:"column:run_id;not null;index:idx_task_items_run" json:"runId"`
	Code   string `gorm:"column:code;type:text;not null" json:"code"`
	Name   string `gorm:"column:name;type:text;not null" json:"name"`
	// Kind 只能是 `daily` / `task` / `checkin`（DB 有 CHECK 约束）。
	Kind       string    `gorm:"column:kind;type:text;not null" json:"kind"`
	Status     string    `gorm:"column:status;type:text;not null" json:"status"`
	Message    string    `gorm:"column:message;type:text;not null;default:''" json:"message"`
	Earned     int       `gorm:"column:earned;type:integer;not null;default:0" json:"earned"`
	FinishedAt time.Time `gorm:"column:finished_at;not null;default:now()" json:"finishedAt"`
}

func (TaskRunItems) TableName() string { return "task_run_items" }

// Settings 是键值配置。`value` 是 jsonb，能存标量也能存数组。
type Settings struct {
	Key       string    `gorm:"column:key;primaryKey;type:text" json:"key"`
	Value     JSONB     `gorm:"column:value;type:jsonb;not null;default:'{}'" json:"value"`
	UpdatedAt time.Time `gorm:"column:updated_at;not null;default:now()" json:"updatedAt"`
}

func (Settings) TableName() string { return "settings" }

// Proxies 是出站代理。密码加密后存 `password_enc`。
type Proxies struct {
	ID        int64  `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	Name      string `gorm:"column:name;type:text;not null" json:"name"`
	Protocol  string `gorm:"column:protocol;type:text;not null" json:"protocol"`
	Host      string `gorm:"column:host;type:text;not null" json:"host"`
	Port      int    `gorm:"column:port;type:integer;not null" json:"port"`
	Username  string `gorm:"column:username;type:text;not null;default:''" json:"username"`
	PasswordEnc string `gorm:"column:password_enc;type:text;not null;default:''" json:"-"`
	// Fallback 是失败时的降级策略（`direct` / `none` 等）。
	Fallback  string    `gorm:"column:fallback;type:text;not null;default:''" json:"fallback"`
	Enabled   bool      `gorm:"column:enabled;not null;default:true" json:"enabled"`
	Note      string    `gorm:"column:note;type:text;not null;default:''" json:"note"`
	CreatedAt time.Time `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
	UpdatedAt time.Time `gorm:"column:updated_at;not null;default:now()" json:"updatedAt"`
}

func (Proxies) TableName() string { return "proxies" }

// RedeemCodes 是充值码。
type RedeemCodes struct {
	ID        int64      `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	Code      string     `gorm:"column:code;type:text;not null;uniqueIndex:idx_redeem_code" json:"code"`
	Amount    float64    `gorm:"column:amount;type:numeric(14,8);not null" json:"amount"`
	Note      string     `gorm:"column:note;type:text;not null;default:''" json:"note"`
	ExpiresAt *time.Time `gorm:"column:expires_at" json:"expiresAt"`
	UsedBy    *int64     `gorm:"column:used_by" json:"usedBy"`
	UsedAt    *time.Time `gorm:"column:used_at" json:"usedAt"`
	CreatedAt time.Time  `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
}

func (RedeemCodes) TableName() string { return "redeem_codes" }

// BalanceTransactions 是余额流水。**只追加**。
type BalanceTransactions struct {
	ID           int64   `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	UserID       int64   `gorm:"column:user_id;not null;index:idx_balance_user" json:"userId"`
	Amount       float64 `gorm:"column:amount;type:numeric(14,8);not null" json:"amount"`
	// BalanceAfter 是这笔之后的余额，让流水可以独立对账。
	BalanceAfter float64 `gorm:"column:balance_after;type:numeric(14,8);not null" json:"balanceAfter"`
	// Kind：`redeem` / `adjust` / `usage`。
	Kind    string    `gorm:"column:kind;type:text;not null" json:"kind"`
	RefID   *int64    `gorm:"column:ref_id" json:"refId"`
	Note    string    `gorm:"column:note;type:text;not null;default:''" json:"note"`
	CreatedAt time.Time `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
}

func (BalanceTransactions) TableName() string { return "balance_transactions" }

// UserSessions 是登录令牌（只存哈希）。
type UserSessions struct {
	ID        int64     `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	UserID    int64     `gorm:"column:user_id;not null;index:idx_sessions_user" json:"userId"`
	TokenHash string    `gorm:"column:token_hash;type:text;not null;uniqueIndex:idx_sessions_token" json:"-"`
	ExpiresAt time.Time `gorm:"column:expires_at;not null" json:"expiresAt"`
	CreatedAt time.Time `gorm:"column:created_at;not null;default:now()" json:"createdAt"`
}

func (UserSessions) TableName() string { return "user_sessions" }

// AffinityBindings 是会话粘滞：同一对话尽量打到同一个号。
type AffinityBindings struct {
	Key           string `gorm:"column:key;primaryKey;type:text" json:"key"`
	ChannelID     string `gorm:"column:channel_id;type:text;not null" json:"channelId"`
	UID           string `gorm:"column:uid;type:text;not null" json:"uid"`
	UpstreamModel string `gorm:"column:upstream_model;type:text;not null" json:"upstreamModel"`
	ExpiresAtMS   int64  `gorm:"column:expires_at_ms;not null;index:idx_affinity_expires" json:"expiresAtMs"`
	Hits          int    `gorm:"column:hits;type:integer;not null;default:0" json:"hits"`
}

func (AffinityBindings) TableName() string { return "affinity_bindings" }

// ChannelCaps 是渠道的运营能力缓存（任务/每日动作/签到）。
type ChannelCaps struct {
	ChannelID string    `gorm:"column:channel_id;primaryKey;type:text" json:"channelId"`
	Tasks     bool      `gorm:"column:tasks;not null;default:false" json:"tasks"`
	Daily     bool      `gorm:"column:daily;not null;default:false" json:"daily"`
	Checkin   bool      `gorm:"column:checkin;not null;default:false" json:"checkin"`
	UpdatedAt time.Time `gorm:"column:updated_at;not null;default:now()" json:"updatedAt"`
}

func (ChannelCaps) TableName() string { return "channel_caps" }
