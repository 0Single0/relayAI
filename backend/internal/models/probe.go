package models

import "time"

/*
探活（定时心跳）相关表。

为什么按**分组 × 模型**而不是按账号：

  上游的限流与权限按模型生效。一个分组挂了 19 个模型，
  只发一句 hello 得不出"这个分组能不能用"——某个模型挂了、
  某个号对该模型无权限，探针照样是绿的。

  参考实现（sub2api）也是这个口径：监控是独立实体，
  带 `primary_model` / `extra_models`，结果按模型分别落库。

  所以这里记到 (channel_id, model) 粒度，而不是 (account_uid)。
*/

// ChannelProbes 是一次探活结果。
//
// 只记**结论**，不记请求/响应原文：探活会消耗上游额度，
// 但它的价值在于"这个模型现在通不通"，不在于回显了什么。
// 存原文会把这张表撑大几十倍，且含凭据相关的上下文。
type ChannelProbes struct {
	ID int64 `gorm:"column:id;primaryKey;autoIncrement" json:"id"`
	// ChannelID 是被探的分组。
	ChannelID string `gorm:"column:channel_id;type:text;not null;index:idx_probes_channel_time,priority:1" json:"channelId"`
	// Model 是**上游模型名**。
	//
	// 用上游名而不是对外名：同一对外名可能被不同账号映射到不同的上游模型，
	// 而限流是按上游名记的（与网关的 model_cooldowns 同一口径）。
	Model string `gorm:"column:model;type:text;not null;index:idx_probes_channel_time,priority:2" json:"model"`
	// OutboundModel 是对外模型名（用于界面展示）。
	OutboundModel string `gorm:"column:outbound_model;type:text;not null;default:''" json:"outboundModel"`
	// AccountUID 是实际被探的账号（记录"这次是哪个号跑的"）。
	AccountUID string `gorm:"column:account_uid;type:text;not null;default:''" json:"accountUid"`
	// OK 表示这次探测是否通。
	OK bool `gorm:"column:ok;not null;default:false" json:"ok"`
	// LatencyMS 是端到端耗时。
	LatencyMS int `gorm:"column:latency_ms;not null;default:0" json:"latencyMs"`
	// Status 是归类后的状态：`ok` / `limited` / `auth` / `quota` / `error`。
	//
	// 单独一列而不是从 OK 推：失败的原因决定"要不要换号"，
	// 全归成"失败"等于把可行动信息丢了。
	Status string `gorm:"column:status;type:text;not null;default:'error'" json:"status"`
	// Message 是这次结果的一句话说明（限流多久、缺什么权限…）。
	Message string `gorm:"column:message;type:text;not null;default:''" json:"message"`
	// Trigger 区分手动与定时：`manual` / `schedule`。
	Trigger string `gorm:"column:trigger;type:text;not null;default:'schedule'" json:"trigger"`
	// CheckedAt 是探测时刻。
	CheckedAt time.Time `gorm:"column:checked_at;not null;default:now();index:idx_probes_channel_time,priority:3" json:"checkedAt"`
}

func (ChannelProbes) TableName() string { return "channel_probes" }
