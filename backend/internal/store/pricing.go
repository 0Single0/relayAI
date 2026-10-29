package store

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	dbmodels "relayai/backend/internal/models"
)

// ModelPrice 是一个模型的价格（元 / 百万 token）。
//
// 三个价分开存而不是合成一个：缓存读取的价格通常是输入价的 1/10，
// 合起来就没法算准 —— 而长对话里缓存占输入的比例可以到 80% 以上，
// 那点差价会直接决定毛利。
type ModelPrice struct {
	Input     float64
	Output    float64
	CacheRead float64
}

// ApplyBuiltinPrices 给"从未定价"的模型填入内置价格。
//
// 只填三项单价都为 0 的：人工改过的价格不该被同步覆盖 ——
// 那可能是谈下来的折扣价，被一次同步改回标价会直接影响毛利。
//
// `usdToCny` 是汇率；落库单位是元 / 百万 token。
func (s *Store) ApplyBuiltinPrices(ctx context.Context, usdToCny float64) (int64, error) {
	if usdToCny <= 0 {
		usdToCny = 7.2
	}

	var names []string
	if err := s.db.WithContext(ctx).
		Model(&dbmodels.Models{}).
		Where("input_price = 0 AND output_price = 0 AND cache_read_price = 0").
		Pluck("canonical_name", &names).Error; err != nil {
		return 0, fmt.Errorf("查询未定价模型失败: %w", err)
	}

	var filled int64
	for _, name := range names {
		price, ok := lookupBuiltinPrice(name)
		if !ok {
			continue
		}
		priceCNY := price.inCNY(usdToCny)
		affected, err := s.exec(ctx,
			`UPDATE models SET
			     input_price = $2, output_price = $3, cache_read_price = $4
			 WHERE canonical_name = $1
			   AND input_price = 0 AND output_price = 0 AND cache_read_price = 0`,
			name,
			priceCNY.Input,
			priceCNY.Output,
			priceCNY.CacheRead,
		)
		if err != nil {
			return filled, fmt.Errorf("写入内置价格失败: %w", err)
		}
		filled += affected
	}

	return filled, nil
}

// OpenRouterModel 是 OpenRouter 返回的一条模型（只取需要的字段）。
type OpenRouterModel struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Pricing struct {
		// Prompt / Completion 是**每 token** 的美元价（字符串形式）。
		Prompt     string `json:"prompt"`
		Completion string `json:"completion"`
		// InputCacheRead 是缓存读取价（部分模型才有）。
		InputCacheRead string `json:"input_cache_read"`
	} `json:"pricing"`
}

// ApplyOpenRouterPrices 用 OpenRouter 的价格覆盖模型价格。
//
// 为什么需要它：内置表会过时，而 OpenRouter 是持续维护的公开价格源。
// 但要注意两点：
//  1. 它只有主流商用模型，国内自建/私有部署的模型它没有 ——
//     所以这是**补充**，不是替代人工配置；
//  2. 它的 `id` 是 `vendor/model` 形式（`openai/gpt-4o`），
//     与我们的对外模型名不一定一致，所以按**末段**匹配（`gpt-4o`）。
func (s *Store) ApplyOpenRouterPrices(
	ctx context.Context,
	models []OpenRouterModel,
	usdToCny float64,
) (int64, error) {
	if usdToCny <= 0 {
		usdToCny = 7.2
	}

	// 建索引：`openai/gpt-4o` → 同时登记全名与末段。
	index := make(map[string]OpenRouterModel, len(models)*2)
	for _, item := range models {
		index[strings.ToLower(item.ID)] = item
		if _, _, ok := strings.Cut(item.ID, "/"); ok {
			tail := item.ID[strings.LastIndex(item.ID, "/")+1:]
			index[strings.ToLower(tail)] = item
		}
	}

	var names []string
	if err := s.db.WithContext(ctx).
		Model(&dbmodels.Models{}).
		Pluck("canonical_name", &names).Error; err != nil {
		return 0, fmt.Errorf("查询模型失败: %w", err)
	}

	var updated int64
	for _, name := range names {
		item, ok := index[strings.ToLower(name)]
		if !ok {
			continue
		}

		// OpenRouter 的价格是"每 token 美元"，要 ×1e6 换成"每百万 token"。
		input := parsePerTokenUSD(item.Pricing.Prompt) * 1e6 * usdToCny
		output := parsePerTokenUSD(item.Pricing.Completion) * 1e6 * usdToCny
		cacheRead := parsePerTokenUSD(item.Pricing.InputCacheRead) * 1e6 * usdToCny
		if input <= 0 && output <= 0 {
			continue
		}

		if _, err := s.exec(ctx,
			`UPDATE models SET
			     input_price = $2, output_price = $3,
			     cache_read_price = CASE WHEN $4 > 0 THEN $4 ELSE cache_read_price END
			 WHERE canonical_name = $1`,
			name, input, output, cacheRead,
		); err != nil {
			return updated, fmt.Errorf("写入 OpenRouter 价格失败: %w", err)
		}
		updated++
	}

	return updated, nil
}

// parsePerTokenUSD 解析 OpenRouter 的字符串价格（`"0.0000025"`）。
func parsePerTokenUSD(raw string) float64 {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return 0
	}
	var value float64
	if _, err := fmt.Sscanf(raw, "%g", &value); err != nil {
		return 0
	}
	return value
}

// ModelPricesForGateway 导出「模型 → 价格」给网关。
//
// 网关需要它来算 `upstream_cost`。为什么不让网关自己查库：
// 网关的存储层只读它需要的列，加一张表的读取要连带改连接与缓存策略；
// 而"模型 → 价格"是一份**很小的只读映射**，通过内部接口传一次、
// 网关缓存在内存里更简单，也不影响它现有的加载流程。
func (s *Store) ModelPricesForGateway(ctx context.Context) (map[string]ModelPrice, error) {
	var rows []struct {
		Name      string  `gorm:"column:canonical_name"`
		Input     float64 `gorm:"column:input_price"`
		Output    float64 `gorm:"column:output_price"`
		CacheRead float64 `gorm:"column:cache_read_price"`
	}
	if err := s.queryAll(ctx, &rows,
		`SELECT canonical_name, input_price::float8 AS input_price,
		        output_price::float8 AS output_price, cache_read_price::float8 AS cache_read_price
		 FROM models WHERE input_price > 0 OR output_price > 0`); err != nil {
		return nil, fmt.Errorf("查询模型价格失败: %w", err)
	}

	out := make(map[string]ModelPrice, len(rows))
	for _, row := range rows {
		out[row.Name] = ModelPrice{Input: row.Input, Output: row.Output, CacheRead: row.CacheRead}
	}
	return out, nil
}

// 确保 json 被使用（OpenRouterModel 的标签需要它）。
var _ = json.Marshal
