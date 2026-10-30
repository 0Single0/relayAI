package store

import (
	"sort"

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

// probeModelUnion 合并 `probe_models` 与旧版 `probe_model` 单值。
//
// 口径必须与定时探活（`ProbeTargets` 的 SQL 并集）完全一致：
// 界面列的是这一份，调度器探的是那一份，两边不同就会出现
// "卡片上写着探 A，实际探的是 A+B"这种查不出原因的问题。
//
// 去重并排序，保证同一份数据每次得到的顺序都一样 —— 顺序变了会让
// 卡片上的文字跳动，用户会以为设置在变。
func probeModelUnion(models []string, single string) []string {
	seen := make(map[string]bool, len(models)+1)
	out := make([]string, 0, len(models)+1)
	for _, name := range models {
		if name == "" || seen[name] {
			continue
		}
		seen[name] = true
		out = append(out, name)
	}
	if single != "" && !seen[single] {
		out = append(out, single)
	}
	sort.Strings(out)
	return out
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