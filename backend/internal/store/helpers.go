package store

import (
	"relayai/backend/internal/models"

	"gorm.io/gorm/clause"
)

/*
本文件放仓储层的共用小工具。

为什么不散落在各文件：`emptyToNil` / `nilIfEmpty` 这些语义约定
（"空数组 = 清空限制"、"空串 = 存 NULL"）在多处被依赖，
分散定义时最容易出现"某处改了另一处没改"的不一致。
*/

// emptyToNil 把空切片转成 nil。
//
// 用途：`text[]` 列上 NULL 与 `{}` 语义不同 ——
// NULL 表示"不限制"，`{}` 表示"限制但一个都不允许"。
// 只想要"不限制"的场合统一走这里，避免两种表示并存。
func emptyToNil(items []string) []string {
	if len(items) == 0 {
		return nil
	}
	return items
}

// nilIfEmpty 把空串转成 nil，供可空文本列使用。
func nilIfEmpty(value string) *string {
	if value == "" {
		return nil
	}
	return &value
}

// onConflictUpdateAll 生成 `ON CONFLICT (主键) DO UPDATE SET 其余列 = EXCLUDED.…`。
//
// 用 GORM 的 clause 而不是手写 SQL：列名从模型推导，
// 加字段时不必回来改这段 —— 手写版本漏改过一次，导致新列不更新。
func onConflictUpdateAll() clause.OnConflict {
	return clause.OnConflict{
		Columns:   []clause.Column{{Name: "channel_id"}},
		DoUpdates: clause.AssignmentColumns([]string{"tasks", "daily", "checkin", "updated_at"}),
	}
}

// capsInput 是能力缓存的中间结构，供事务内构造。
type capsInput = models.ChannelCaps

// joinSets 用逗号拼接 SET 片段（动态更新时用）。
func joinSets(sets []string) string {
	out := ""
	for index, item := range sets {
		if index > 0 {
			out += ", "
		}
		out += item
	}
	return out
}