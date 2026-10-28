package store

import (
	"context"
	"encoding/json"
	"fmt"
)

// CatalogModelInput 是"从网关目录同步一个模型"的输入。
//
// 与 `ModelInput`（人工编辑）分开：那个是全指针、允许只改一个字段；
// 这个是整批同步，字段语义不同（没有价格），混用会让两边都变难读。
type CatalogModelInput struct {
	CanonicalName string
	DisplayName   string
	ContextWindow *int
	MaxOutput     *int
	Capabilities  json.RawMessage
}

// UpsertCatalogModel 按对外模型名写入/更新模型目录。
//
// **只更新目录字段**（上下文、能力、最大输出）；价格、倍率与启用状态保持原值 ——
// 那些是人工配置，不该被一次同步覆盖掉。
// 有人手动停用了某个模型，同步后应当仍是停用。
func (s *Store) UpsertCatalogModel(ctx context.Context, model CatalogModelInput) error {
	capabilities := model.Capabilities
	if len(capabilities) == 0 {
		capabilities = json.RawMessage(`{}`)
	}

	const query = `
		INSERT INTO models (
		    canonical_name, display_name, context_window, max_output, capabilities,
		    input_price, output_price, cache_read_price, model_ratio, enabled
		) VALUES ($1, $2, $3, $4, $5, 0, 0, 0, 1, true)
		ON CONFLICT (canonical_name) DO UPDATE SET
		    display_name = COALESCE(NULLIF(EXCLUDED.display_name, ''), models.display_name),
		    context_window = COALESCE(EXCLUDED.context_window, models.context_window),
		    max_output = COALESCE(EXCLUDED.max_output, models.max_output),
		    capabilities = EXCLUDED.capabilities
		-- 刻意**不动** input_price / output_price / model_ratio / enabled：
		-- 同步只负责"有哪些模型、多长上下文"，定价与启停是人的决定。
		`

	_, err := s.pool.Exec(ctx, query,
		model.CanonicalName, model.DisplayName,
		model.ContextWindow, model.MaxOutput, capabilities,
	)
	if err != nil {
		return fmt.Errorf("写入模型目录失败: %w", err)
	}
	return nil
}

// DeleteModelsExcept 删除不在保留清单里的模型。
//
// 用途：上游把某个模型下线后，同步应当把它从目录里去掉，
// 否则模型广场会一直展示一个用不了的模型。
//
// 空清单时**什么都不删**：网关暂时不可用会返回空列表，
// 那种情况下清空整张表是最糟的处理（用户会以为模型全没了）。
func (s *Store) DeleteModelsExcept(ctx context.Context, keep []string) (int64, error) {
	if len(keep) == 0 {
		return 0, nil
	}

	result, err := s.pool.Exec(
		ctx,
		"DELETE FROM models WHERE canonical_name <> ALL($1)",
		keep,
	)
	if err != nil {
		return 0, fmt.Errorf("清理下线模型失败: %w", err)
	}
	return result.RowsAffected(), nil
}
