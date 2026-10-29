package store

import (
	"encoding/json"
	"time"

	"relayai/backend/internal/models"
)

/*
本文件放"数据库行 → 领域对象"的转换辅助。

为什么需要：
  查询结果里 `jsonb` 列扫成 `[]byte`，而领域对象用 `json.RawMessage`
  或 `map[string]any`。这个转换在十几处重复，集中处理避免每处各写一遍
  空值分支 —— 空 jsonb 返回 `null` 还是 `{}` 曾经不一致过。
*/

// accountRow 是账号查询的行结构。
//
// 用显式结构体 + `gorm:"column:"` 标签，而不是像旧代码那样按位置 Scan：
// 位置扫描在改投影顺序时会静默错位（把 `nickname` 扫进 `uid` 不会报错）。
type accountRow struct {
	ID               int64    `gorm:"column:id"`
	ChannelID        string   `gorm:"column:channel_id"`
	ChannelName      string   `gorm:"column:channel_name"`
	Adapter          string   `gorm:"column:adapter"`
	UID              string   `gorm:"column:uid"`
	Nickname         string   `gorm:"column:nickname"`
	Enabled          bool     `gorm:"column:enabled"`
	PriorityOverride *int     `gorm:"column:priority_override"`
	WeightOverride   *int     `gorm:"column:weight_override"`
	MaxInflight      *int     `gorm:"column:max_inflight"`
	ModelAllow       models.TextArray `gorm:"column:model_allow"`
	ModelRename      []byte   `gorm:"column:model_rename"`
	Source           string   `gorm:"column:source"`
	AddedAt          *time.Time `gorm:"column:added_at"`
	BaseURL          string   `gorm:"column:base_url"`
	Protocol         string   `gorm:"column:protocol"`
	Platform         string   `gorm:"column:platform"`
	Options          []byte   `gorm:"column:options"`
	ProxyID          *int64   `gorm:"column:proxy_id"`
	ProxyName        *string  `gorm:"column:proxy_name"`
}

// rawJSONOrEmpty 把数据库里的 jsonb 字节转成 json.RawMessage。
//
// 空值统一给 `{}`：调用方（前端）拿到 `null` 时要额外判空，
// 而我们所有 jsonb 列的默认值本来就是 `{}`，保持一致。
func rawJSONOrEmpty(raw []byte) json.RawMessage {
	if len(raw) == 0 {
		return json.RawMessage(`{}`)
	}
	return json.RawMessage(raw)
}
// keyRow 是密钥查询的行结构。
type keyRow struct {
	ID              int64      `gorm:"column:id"`
	Name            string     `gorm:"column:name"`
	KeyPrefix       string     `gorm:"column:key_prefix"`
	ChannelAllow    models.TextArray `gorm:"column:channel_allow"`
	ModelAllow      models.TextArray `gorm:"column:model_allow"`
	RPMLimit        *int       `gorm:"column:rpm_limit"`
	DailyTokenLimit *int64     `gorm:"column:daily_token_limit"`
	MaxConcurrency  *int       `gorm:"column:max_concurrency"`
	Enabled         bool       `gorm:"column:enabled"`
	ExpiresAt       *time.Time `gorm:"column:expires_at"`
	LastUsedAt      *time.Time `gorm:"column:last_used_at"`
	CreatedAt       time.Time  `gorm:"column:created_at"`
	KeyPlaintext    *string    `gorm:"column:key_plaintext"`
}

// modelRow 是模型目录查询的行结构。
type modelRow struct {
	CanonicalName  string   `gorm:"column:canonical_name"`
	DisplayName    string   `gorm:"column:display_name"`
	ContextWindow  *int     `gorm:"column:context_window"`
	MaxOutput      *int     `gorm:"column:max_output"`
	Capabilities   []byte   `gorm:"column:capabilities"`
	InputPrice     float64  `gorm:"column:input_price"`
	OutputPrice    float64  `gorm:"column:output_price"`
	CacheReadPrice float64  `gorm:"column:cache_read_price"`
	ModelRatio     float64  `gorm:"column:model_ratio"`
	Enabled        bool     `gorm:"column:enabled"`
}
/*
下面两个是时间列的类型安全格式化。

为什么需要：行结构体里的时间列已改用 *time.Time / 	ime.Time，
而 ormatTime / 
ullableTime 收的是 ny：
传 *time.Time 进去会走 `interface{ Format }` 分支 ——
指针的方法集虽然包含值接收者的 Format，但 **nil 指针会 panic**，
而这两列本来就允许为空（expires_at、last_used_at）。
*/

// formatTimePtr 把可空时间格式化成 RFC3339；nil 返回空串。
func formatTimePtr(value *time.Time) string {
	if value == nil {
		return ""
	}
	return value.Format(time.RFC3339)
}

// formatTimeValue 把非空时间格式化成 RFC3339。
func formatTimeValue(value time.Time) string {
	if value.IsZero() {
		return ""
	}
	return value.Format(time.RFC3339)
}

// nullableTimePtr 把可空时间格式化；nil 或零值返回 nil。
func nullableTimePtr(value *time.Time) *string {
	if value == nil || value.IsZero() {
		return nil
	}
	formatted := value.Format(time.RFC3339)
	return &formatted
}
// orEmptyText 把 nil 数组转成空切片。
//
// 为什么必须做：	ext[] 列允许为 NULL，扫进 TextArray 后是 nil。
// 前端按 rr[0] 读第一项时会因为 null 直接崩页面
// （实测 /keys 整页白屏）。空切片让前端不必到处判空。
func orEmptyText(values models.TextArray) []string {
	if values == nil {
		return []string{}
	}
	return values
}