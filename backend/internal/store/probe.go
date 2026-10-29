package store

import (
	"context"
	"fmt"

	"relayai/backend/internal/models"
)

/*
探活结果与"该探哪些模型"的读取。

为什么这些放在 Store 而不是 handler：
  探活的**选模型**规则要跟调度口径一致（分组 exposed ∩ 账号白名单），
  而那份规则本来就是仓储层的事。放 handler 里会让两条路径慢慢分叉。
*/

// ProbeTarget 是一个待探的目标：某分组的某个模型，用某个账号跑。
type ProbeTarget struct {
	ChannelID string
	AccountUID string
	// Model 是**上游模型名**（网关按它记限流）。
	Model string
	// OutboundModel 是对外名，仅用于展示。
	OutboundModel string
}

// ProbeTargets 返回本轮要探的目标列表。
//
// 规则（**没有默认值，人不配就不探**）：
//  1. 只取**启用**的分组与账号；
//  2. 每个分组**只探它的 `probe_models` 里列出的模型**；
//     列表为空 = 这个分组不探 —— 绝不擅自扩大范围。
//     探活消耗真实额度，"没说要探"和"探全部"必须严格区分，
//     否则一次同步进来的几十个模型会在后台悄悄烧钱。
//  3. 每个模型挑**一个**账号去探 —— 探活回答的是"这个模型通不通"，
//     号级别的可用性由网关的冷却状态反映，不必重复确认。
func (s *Store) ProbeTargets(ctx context.Context) ([]ProbeTarget, error) {
		/*
		一次查询取全：分组 → 账号 → 模型。

		用 SQL 聚合而不是在 Go 里循环查，是因为"每个分组挑一个号"
		需要按分组分组取首行 —— 分三次查会变成 N+1。

		策略：每个 (channel, model) 取一个 uid（min(uid) 保证稳定，
		不随行顺序变化）。
	*/

	const query = `
		WITH enabled_accounts AS (
		    SELECT channel_id, uid, platform,
		           COALESCE(model_allow, ARRAY[]::text[]) AS model_allow
		    FROM accounts
		    WHERE enabled
		),
		/*
			只探 probe_models 里**显式列出**的模型。

			WHERE array_length(...) > 0：没配列表的分组在这里产生零行 ——
			不是"回退到全部模型"。探活花的是真金白银的额度，
			没有人的授权就不动。
		*/
		channel_models AS (
		    SELECT c.id AS channel_id,
		           c.probe_models AS probe_models
		    FROM channels c
		    WHERE c.enabled
		      AND array_length(c.probe_models, 1) > 0
		),
		picked AS (
		    SELECT channel_id, model,
		           (SELECT uid FROM enabled_accounts a
		             WHERE a.channel_id = cm.channel_id
		             ORDER BY a.uid LIMIT 1) AS uid
		    FROM channel_models cm
		    CROSS JOIN LATERAL unnest(cm.probe_models) AS model
		)
		SELECT channel_id,
		       COALESCE(uid, '') AS account_uid,
		       model AS model,
		       model AS outbound_model
		FROM picked
		WHERE COALESCE(uid, '') <> ''
		ORDER BY channel_id, model`

	var rows []struct {
		ChannelID     string `gorm:"column:channel_id"`
		AccountUID    string `gorm:"column:account_uid"`
		Model         string `gorm:"column:model"`
		OutboundModel string `gorm:"column:outbound_model"`
	}
	if err := s.queryAll(ctx, &rows, query); err != nil {
		return nil, fmt.Errorf("查询探活目标失败: %w", err)
	}

	targets := make([]ProbeTarget, 0, len(rows))
	for _, row := range rows {
		targets = append(targets, ProbeTarget{
			ChannelID:     row.ChannelID,
			AccountUID:    row.AccountUID,
			Model:         row.Model,
			OutboundModel: row.OutboundModel,
		})
	}
	return targets, nil
}

// SaveProbe 落一条探活结果。
func (s *Store) SaveProbe(ctx context.Context, item models.ChannelProbes) error {
	if err := s.db.WithContext(ctx).Create(&item).Error; err != nil {
		return fmt.Errorf("保存探活结果失败: %w", err)
	}
	return nil
}

// LatestProbes 返回每个 (分组, 模型) 的**最新一条**探活结果。
//
// 界面只关心"现在怎么样"，历史用于画趋势，不该在列表页全量捞。
func (s *Store) LatestProbes(ctx context.Context) ([]models.ChannelProbes, error) {
	const query = `
		SELECT DISTINCT ON (channel_id, model)
		       id, channel_id, model, outbound_model, account_uid,
		       ok, latency_ms, status, message, trigger, checked_at
		FROM channel_probes
		ORDER BY channel_id, model, checked_at DESC`

	var rows []models.ChannelProbes
	if err := s.queryAll(ctx, &rows, query); err != nil {
		return nil, fmt.Errorf("查询最新探活失败: %w", err)
	}
	return rows, nil
}

// ProbeHistory 返回某分组最近的探活记录（按时间倒序）。
//
// `limit` 上限 500：界面只画最近一段趋势，取太多没有意义还把响应撑大。
func (s *Store) ProbeHistory(ctx context.Context, channelID string, limit int) ([]models.ChannelProbes, error) {
	if limit <= 0 || limit > 500 {
		limit = 200
	}
	list := make([]models.ChannelProbes, 0, limit)
	if err := s.db.WithContext(ctx).
		Where("channel_id = ?", channelID).
		Order("checked_at DESC").
		Limit(limit).
		Find(&list).Error; err != nil {
		return nil, fmt.Errorf("查询探活历史失败: %w", err)
	}
	return list, nil
}
