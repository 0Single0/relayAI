package store

import (
	"context"
	"fmt"
	"time"
)

// KeyUsage 是一个 API Key 的累计用量。
//
// 为什么单独查而不是塞进 `ListKeys`：
// 用量要扫整张 `usage_logs`（千万行级别），而 Key 列表是秒开的页面。
// 把聚合塞进列表查询会让"打开密钥管理"变成全表扫描。
//
// 现在是一次独立的 GROUP BY 查询 —— 同样扫全表，但只扫一遍、只返回几十行，
// 而且不阻塞列表的其它字段。
type KeyUsage struct {
	// 今日
	TodayRequests int64   `json:"todayRequests"`
	TodayTokens   int64   `json:"todayTokens"`
	TodayCost     float64 `json:"todayCost"`
	// 累计
	TotalRequests int64   `json:"totalRequests"`
	TotalTokens   int64   `json:"totalTokens"`
	TotalCost     float64 `json:"totalCost"`
	// 最近一次使用；nil 表示从未用过。
	LastUsed *time.Time `json:"lastUsed"`
}

// ListKeyUsage 按 Key 聚合用量。
//
// 口径：
//   - `billed_cost` 是**记账额度**（含倍率），不是上游成本 ——
//     用户看到的"消费"必须是这个，否则和账单对不上；
//   - 拒绝行（`is_rejection`）不计入：那是被挡下的请求，没有产生费用；
//   - 今日按服务器本地时区的自然日切分。
//
// 为什么这段仍是手写 SQL：
// 它用了 Postgres 的 `FILTER (WHERE ...)` 做单遍条件聚合，还要把
// numeric 转 float8。GORM 的链式 API 表达不了这种"一次扫描算六个数"的语义，
// 用 `Select("count(*) FILTER ...")` 拼字符串读起来更差、也更容易出错。
// 用 `Raw` 保留 SQL、交给 GORM 执行与扫描 —— 既统一在 GORM 机制下，
// 又保住了这段本来就该手写的聚合。
func (s *Store) ListKeyUsage(ctx context.Context) (map[int64]KeyUsage, error) {
	const query = `
		SELECT key_id,
		       count(*) FILTER (WHERE ts >= date_trunc('day', now()))                       AS today_requests,
		       COALESCE(sum(total_tokens) FILTER (WHERE ts >= date_trunc('day', now())), 0) AS today_tokens,
		       COALESCE(sum(billed_cost) FILTER (WHERE ts >= date_trunc('day', now())), 0)::float8 AS today_cost,
		       count(*)                                                                     AS total_requests,
		       COALESCE(sum(total_tokens), 0)                                               AS total_tokens,
		       COALESCE(sum(billed_cost), 0)::float8                                        AS total_cost,
		       max(ts)                                                                      AS last_used
		FROM usage_logs
		WHERE NOT is_rejection AND key_id IS NOT NULL
		GROUP BY key_id`

	// 行结构与上面的投影**顺序对应**，字段名用 gorm 的 `column` 标签对齐。
	type row struct {
		KeyID         int64      `gorm:"column:key_id"`
		TodayRequests int64      `gorm:"column:today_requests"`
		TodayTokens   int64      `gorm:"column:today_tokens"`
		TodayCost     float64    `gorm:"column:today_cost"`
		TotalRequests int64      `gorm:"column:total_requests"`
		TotalTokens   int64      `gorm:"column:total_tokens"`
		TotalCost     float64    `gorm:"column:total_cost"`
		LastUsed      *time.Time `gorm:"column:last_used"`
	}

	var rows []row
	if err := s.db.WithContext(ctx).Raw(query).Scan(&rows).Error; err != nil {
		return nil, fmt.Errorf("查询密钥用量失败: %w", err)
	}

	result := make(map[int64]KeyUsage, len(rows))
	for _, item := range rows {
		result[item.KeyID] = KeyUsage{
			TodayRequests: item.TodayRequests,
			TodayTokens:   item.TodayTokens,
			TodayCost:     item.TodayCost,
			TotalRequests: item.TotalRequests,
			TotalTokens:   item.TotalTokens,
			TotalCost:     item.TotalCost,
			LastUsed:      item.LastUsed,
		}
	}
	return result, nil
}
