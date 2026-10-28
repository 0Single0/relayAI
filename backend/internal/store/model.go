package store

import (
	"context"
	"encoding/json"
	"fmt"
)

// Model 是模型目录与价格。
type Model struct {
	CanonicalName  string          `json:"canonicalName"`
	DisplayName    string          `json:"displayName"`
	ContextWindow  *int            `json:"contextWindow"`
	MaxOutput      *int            `json:"maxOutput"`
	Capabilities   json.RawMessage `json:"capabilities"`
	InputPrice     float64         `json:"inputPrice"`
	OutputPrice    float64         `json:"outputPrice"`
	CacheReadPrice float64         `json:"cacheReadPrice"`
	ModelRatio     float64         `json:"modelRatio"`
	Enabled        bool            `json:"enabled"`
	Priced         bool            `json:"priced"`
}

// ListModels 返回模型目录。
func (s *Store) ListModels(ctx context.Context) ([]Model, error) {
	const query = `
		SELECT canonical_name, display_name, context_window, max_output,
		       capabilities,
		       input_price::float8, output_price::float8, cache_read_price::float8,
		       model_ratio::float8, enabled
		FROM models ORDER BY canonical_name`

	rows, err := s.pool.Query(ctx, query)
	if err != nil {
		return nil, fmt.Errorf("查询模型失败: %w", err)
	}
	defer rows.Close()

	list := make([]Model, 0, 16)
	for rows.Next() {
		var model Model
		if err := rows.Scan(
			&model.CanonicalName, &model.DisplayName, &model.ContextWindow, &model.MaxOutput,
			&model.Capabilities,
			&model.InputPrice, &model.OutputPrice, &model.CacheReadPrice,
			&model.ModelRatio, &model.Enabled,
		); err != nil {
			return nil, fmt.Errorf("扫描模型失败: %w", err)
		}
		// "已定价"= 至少有一个价格大于 0；全零视为未定价，界面要提示。
		model.Priced = model.InputPrice > 0 || model.OutputPrice > 0
		list = append(list, model)
	}

	return list, rows.Err()
}

// ModelInput 是更新模型的输入。
type ModelInput struct {
	DisplayName    *string         `json:"displayName"`
	ContextWindow  *int            `json:"contextWindow"`
	MaxOutput      *int            `json:"maxOutput"`
	Capabilities   json.RawMessage `json:"capabilities"`
	InputPrice     *float64        `json:"inputPrice"`
	OutputPrice    *float64        `json:"outputPrice"`
	CacheReadPrice *float64        `json:"cacheReadPrice"`
	ModelRatio     *float64        `json:"modelRatio"`
	Enabled        *bool           `json:"enabled"`
}

// UpdateModel 更新模型（不存在则插入，便于给"未定价"的模型补价格）。
//
// **价格参数必须显式 `::numeric`**：pgx 把 `*float64` 编码成 `float8`，
// 而 `COALESCE(float8, 0)` 里的整数 `0` 会让 Postgres 把结果推断成 integer，
// 于是 0.8 被**向零取整成 0**、2.8 变 2 —— 表现为"填了价格保存后又变回去"。
// 这不是精度问题（列是 numeric(14,8)），而是 COALESCE 的类型统一规则。
func (s *Store) UpdateModel(ctx context.Context, name string, input ModelInput) (*Model, error) {
	const query = `
		INSERT INTO models (canonical_name, display_name, context_window, max_output,
		                    capabilities, input_price, output_price, cache_read_price,
		                    model_ratio, enabled)
		VALUES ($1, COALESCE($2, ''), $3, $4, COALESCE($5, '{}'::jsonb),
		        COALESCE($6::numeric, 0), COALESCE($7::numeric, 0), COALESCE($8::numeric, 0),
		        COALESCE($9::numeric, 1.0), COALESCE($10, true))
		ON CONFLICT (canonical_name) DO UPDATE SET
		    display_name = COALESCE($2, models.display_name),
		    context_window = COALESCE($3, models.context_window),
		    max_output = COALESCE($4, models.max_output),
		    capabilities = COALESCE($5, models.capabilities),
		    input_price = COALESCE($6::numeric, models.input_price),
		    output_price = COALESCE($7::numeric, models.output_price),
		    cache_read_price = COALESCE($8::numeric, models.cache_read_price),
		    model_ratio = COALESCE($9::numeric, models.model_ratio),
		    enabled = COALESCE($10, models.enabled)
		RETURNING canonical_name, display_name, context_window, max_output,
		          capabilities, input_price::float8, output_price::float8,
		          cache_read_price::float8, model_ratio::float8, enabled`

	var model Model
	err := s.pool.QueryRow(ctx, query,
		name, input.DisplayName, input.ContextWindow, input.MaxOutput,
		nullableJSON(input.Capabilities), input.InputPrice, input.OutputPrice,
		input.CacheReadPrice, input.ModelRatio, input.Enabled,
	).Scan(
		&model.CanonicalName, &model.DisplayName, &model.ContextWindow, &model.MaxOutput,
		&model.Capabilities, &model.InputPrice, &model.OutputPrice,
		&model.CacheReadPrice, &model.ModelRatio, &model.Enabled,
	)
	if err != nil {
		return nil, fmt.Errorf("更新模型失败: %w", err)
	}

	model.Priced = model.InputPrice > 0 || model.OutputPrice > 0
	return &model, nil
}

func nullableJSON(value json.RawMessage) any {
	if len(value) == 0 {
		return nil
	}
	return value
}
