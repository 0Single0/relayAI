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

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询密钥用量失败: %w", err)
	}
	defer rows.Close()

	result := make(map[int64]KeyUsage, 32)
	for rows.Next() {
		var keyID int64
		var item KeyUsage
		var lastUsed *time.Time
		if err := rows.Scan(
			&keyID,
			&item.TodayRequests, &item.TodayTokens, &item.TodayCost,
			&item.TotalRequests, &item.TotalTokens, &item.TotalCost,
			&lastUsed,
		); err != nil {
			return nil, fmt.Errorf("扫描密钥用量失败: %w", err)
		}
		item.LastUsed = lastUsed
		result[keyID] = item
	}

	return result, rows.Err()
}
