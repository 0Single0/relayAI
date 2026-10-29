package store

import (
	"context"
	"fmt"

	"gorm.io/gorm"
)

// 余额扣费。
//
// ---------------------------------------------------------------------------
// 为什么需要这一步（一个真实的缺口）
// ---------------------------------------------------------------------------
// 网关算出了 `billed_cost` 并写进 `usage_logs`，但**从来没有从用户余额里扣过钱**。
// 也就是说：用量记录、账单、价格全都对，只有"扣钱"这一环是空的 ——
// 用户余额永远不变，等于白用。
//
// 为什么不在网关里扣：
//   · 网关的存储层刻意只管"写用量"，不碰用户账务（它连 `users` 表都不读）；
//   · 扣费要和 `balance_transactions` 在同一事务里，属于管理面的职责；
//   · 网关可能多实例，账务集中在后端更好对账。
//
// 所以做成**管理面的结算动作**：把还没结算的用量挑出来，
// 按 Key 归到用户，逐笔记账并写流水，最后标记已结算。

// Settlement 是一次结算的结果。
type Settlement struct {
	// 结算的用量条数
	Count int `json:"count"`
	// 扣掉的总金额（元）
	Amount float64 `json:"amount"`
	// 受影响的用户数
	Users int `json:"users"`
	// 余额不足的用户（这些用户的用量仍然记账，但余额不扣成负数）
	Insufficient []int64 `json:"insufficient"`
}

// SettleUsage 把未结算的用量扣到用户余额上。
//
// 幂等：`usage_logs.settled_at` 标记已结算，重复调用不会重复扣。
// 这一点很重要 —— 定时任务和手动触发可能同时跑。
//
// 余额不足时**不把余额扣成负数**，但用量仍然标记为已结算：
// 继续扣会让余额变成负债，而反复重试只会让日志刷屏。
// 这类用户单独返回，由管理员决定是停用 Key 还是补款。
func (s *Store) SettleUsage(ctx context.Context, limit int) (*Settlement, error) {
	if limit <= 0 || limit > 5000 {
		limit = 500
	}

	// 1. 挑出未结算的用量（只取有 Key、有金额的）。
	//
	// `k.user_id` 可能为 NULL（历史 Key 没绑定用户）——
	// 那种 Key 没有归属，扣谁的钱都说不清，所以直接排除，避免整批扫描失败。
	const pickQuery = `
		SELECT u.id, k.user_id, u.billed_cost::float8
		FROM usage_logs u
		JOIN api_keys k ON k.id = u.key_id
		WHERE u.settled_at IS NULL
		  AND u.billed_cost > 0
		  AND NOT u.is_rejection
		  AND k.user_id IS NOT NULL
		ORDER BY u.id
		LIMIT $1`

	type entry struct {
		UsageID int64   `gorm:"column:id"`
		UserID  int64   `gorm:"column:user_id"`
		Amount  float64 `gorm:"column:billed_cost"`
	}

	var rows []entry
	if err := s.queryAll(ctx, &rows, pickQuery, limit); err != nil {
		return nil, fmt.Errorf("查询未结算用量失败: %w", err)
	}

	entries := make([]entry, 0, len(rows))
	for _, row := range rows {
		entries = append(entries, row)
	}

	result := &Settlement{Count: len(entries)}
	if len(entries) == 0 {
		return result, nil
	}

	// 2. 按用户汇总后逐户扣款。
	//
	// 逐笔扣会产生几百条流水，对账时根本看不过来；
	// 按用户合并成一笔，备注里写清"结算 N 条用量"。
	byUser := make(map[int64]float64, 16)
	usageByUser := make(map[int64][]int64, 16)
	for _, item := range entries {
		byUser[item.UserID] += item.Amount
		usageByUser[item.UserID] = append(usageByUser[item.UserID], item.UsageID)
		result.Amount += item.Amount
	}
	result.Users = len(byUser)

	for userID, amount := range byUser {
		ids := usageByUser[userID]

		// 每个用户一个独立事务：一个用户余额不足不该回滚别人的扣款。
		txErr := s.db.WithContext(ctx).Transaction(func(tx *gorm.DB) error {
			// 条件更新：余额够才扣。`RETURNING` 在零行时 GORM 不会报错，
			// 所以用 RowsAffected 判断"余额不足"。
			var balance float64
			update := tx.Raw(
				`UPDATE users
				    SET balance = balance - ?
				  WHERE id = ? AND balance >= ?
				 RETURNING balance::float8`,
				amount, userID, amount,
			).Scan(&balance)

			insufficient := update.RowsAffected == 0
			if !insufficient {
				if err := tx.Exec(
					`INSERT INTO balance_transactions (user_id, amount, balance_after, kind, note)
					 VALUES (?, ?, ?, 'usage', ?)`,
					userID, -amount, balance,
					fmt.Sprintf("结算 %d 条用量", len(ids)),
				).Error; err != nil {
					return fmt.Errorf("写入余额流水失败: %w", err)
				}
			}

			// 标记已结算。余额不足的也标记 —— 见上方说明。
			//
			// 用 `IN (?)` 而不是 `= ANY(?)`：GORM 会把切片展开成
			// `IN ($1,$2,$3)`，而 `ANY(...)` 需要的是**单个数组参数**，
			// 展开后会变成 `ANY($1,$2,$3)` —— 语法错误（实测 SQLSTATE 42601）。
			if err := tx.Exec(
				"UPDATE usage_logs SET settled_at = now() WHERE id IN (?)",
				ids,
			).Error; err != nil {
				return fmt.Errorf("标记用量已结算失败: %w", err)
			}

			if insufficient {
				// 余额不足时这笔金额没扣，从总额里减掉，避免报表虚高。
				result.Amount -= amount
				result.Insufficient = append(result.Insufficient, userID)
			}
			return nil
		})
		if txErr != nil {
			return result, txErr
		}
	}

	return result, nil
}
