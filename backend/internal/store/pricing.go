package store

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
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

// BuiltinPrices 是内置价格表（**美元 / 百万 token**，按主流厂商公开定价）。
//
// 为什么内置一份：网关同步过来的目录只有模型名与能力，没有价格 ——
// 上游不会告诉你"这个模型多少钱"。没有默认值时，新同步的模型全是"未定价"，
// 计费恒为 0，用户以为系统在记账其实没有。
//
// **价格以官方定价为准，人工维护。** 这些数字来自各厂商公开的定价页。
//
// 为什么不让 OpenRouter 主导：实测它拉下来的价格与官方牌价有偏差
// （它自己的加价、缓存价缺失、以及把一些模型标成离谱的低价/高价）。
// 计费按它算会让账单莫名其妙，所以：
//   - **内置表（官方价）是默认来源**，同步时自动填"从未定价"的模型；
//   - OpenRouter 降级为**参考**，管理员可手动触发，但需要自己复核；
//   - 最终以管理员手工编辑为准。
//
// 币种是美元，落库时由调用方按汇率折算成元 —— 汇率也做成可配置，
// 否则写死一个数字，半年后整张表都偏了。
var BuiltinPrices = map[string]ModelPrice{
	// ---- OpenAI ----
	"gpt-5":            {Input: 1.25, Output: 10, CacheRead: 0.125},
	"gpt-5-mini":       {Input: 0.25, Output: 2, CacheRead: 0.025},
	"gpt-5-nano":       {Input: 0.05, Output: 0.4, CacheRead: 0.005},
	"gpt-4.1":          {Input: 2, Output: 8, CacheRead: 0.5},
	"gpt-4.1-mini":     {Input: 0.4, Output: 1.6, CacheRead: 0.1},
	"gpt-4.1-nano":     {Input: 0.1, Output: 0.4, CacheRead: 0.025},
	"gpt-4o":           {Input: 2.5, Output: 10, CacheRead: 1.25},
	"gpt-4o-mini":      {Input: 0.15, Output: 0.6, CacheRead: 0.075},
	"o3":               {Input: 2, Output: 8, CacheRead: 0.5},
	"o3-mini":          {Input: 1.1, Output: 4.4, CacheRead: 0.55},
	"o4-mini":          {Input: 1.1, Output: 4.4, CacheRead: 0.275},

	// ---- Anthropic ----
	"claude-opus-4":     {Input: 15, Output: 75, CacheRead: 1.5},
	"claude-sonnet-4":   {Input: 3, Output: 15, CacheRead: 0.3},
	"claude-3-7-sonnet": {Input: 3, Output: 15, CacheRead: 0.3},
	"claude-3-5-sonnet": {Input: 3, Output: 15, CacheRead: 0.3},
	"claude-3-5-haiku":  {Input: 0.8, Output: 4, CacheRead: 0.08},

	// ---- Google ----
	"gemini-2.5-pro":   {Input: 1.25, Output: 10, CacheRead: 0.31},
	"gemini-2.5-flash": {Input: 0.3, Output: 2.5, CacheRead: 0.075},

	// ---- DeepSeek ----
	"deepseek-v3":      {Input: 0.27, Output: 1.1, CacheRead: 0.07},
	"deepseek-v3.1":    {Input: 0.27, Output: 1.1, CacheRead: 0.07},
	"deepseek-r1":      {Input: 0.55, Output: 2.19, CacheRead: 0.14},
	"deepseek-chat":    {Input: 0.27, Output: 1.1, CacheRead: 0.07},
	"deepseek-reasoner": {Input: 0.55, Output: 2.19, CacheRead: 0.14},

	// ---- 智谱 GLM ----
	"glm-4.6":      {Input: 0.6, Output: 2.2, CacheRead: 0.11},
	"glm-4.5":      {Input: 0.6, Output: 2.2, CacheRead: 0.11},
	"glm-4.5-air":  {Input: 0.2, Output: 1.1, CacheRead: 0.03},

	// ---- 月之暗面 Kimi ----
	"kimi-k2":   {Input: 0.6, Output: 2.5, CacheRead: 0.15},
	"kimi-k2.5": {Input: 0.6, Output: 2.5, CacheRead: 0.15},

	// ---- 阿里 Qwen ----
	"qwen3-max":   {Input: 1.2, Output: 6, CacheRead: 0.24},
	"qwen3-plus":  {Input: 0.4, Output: 1.2, CacheRead: 0.08},
	"qwen3-turbo": {Input: 0.05, Output: 0.2, CacheRead: 0.01},
}

// lookupBuiltinPrice 按模型名找内置价格。
//
// 匹配规则：先精确，再**前缀**。上游的模型名常带日期后缀
// （`gpt-4o-mini-2024-07-18`），精确匹配会全部落空 —— 那样内置表等于没用。
// 前缀取最长的那条，避免 `gpt-4o` 抢走 `gpt-4o-mini` 的价格。
func lookupBuiltinPrice(model string) (ModelPrice, bool) {
	name := strings.ToLower(strings.TrimSpace(model))
	if name == "" {
		return ModelPrice{}, false
	}

	if price, ok := BuiltinPrices[name]; ok {
		return price, true
	}

	best := ""
	var bestPrice ModelPrice
	for key, price := range BuiltinPrices {
		if strings.HasPrefix(name, key) && len(key) > len(best) {
			best = key
			bestPrice = price
		}
	}
	if best != "" {
		return bestPrice, true
	}
	return ModelPrice{}, false
}

// ApplyBuiltinPrices 给"从未定价"的模型填入内置价格。
//
// 只填 `input_price = 0` 的：人工改过的价格不该被同步覆盖 ——
// 那可能是谈下来的折扣价，被一次同步改回标价会直接影响毛利。
//
// `usdToCny` 是汇率；落库单位是元 / 百万 token。
func (s *Store) ApplyBuiltinPrices(ctx context.Context, usdToCny float64) (int64, error) {
	if usdToCny <= 0 {
		usdToCny = 7.2
	}

	rows, err := s.pool.Query(ctx,
		"SELECT canonical_name FROM models WHERE input_price = 0 OR output_price = 0")
	if err != nil {
		return 0, fmt.Errorf("查询未定价模型失败: %w", err)
	}
	defer rows.Close()

	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return 0, fmt.Errorf("扫描模型名失败: %w", err)
		}
		names = append(names, name)
	}
	if err := rows.Err(); err != nil {
		return 0, err
	}

	var filled int64
	for _, name := range names {
		price, ok := lookupBuiltinPrice(name)
		if !ok {
			continue
		}
		_, err := s.pool.Exec(ctx,
			`UPDATE models SET
			     input_price = $2, output_price = $3, cache_read_price = $4
			 WHERE canonical_name = $1
			   AND (input_price = 0 OR output_price = 0)`,
			name,
			price.Input*usdToCny,
			price.Output*usdToCny,
			price.CacheRead*usdToCny,
		)
		if err != nil {
			return filled, fmt.Errorf("写入内置价格失败: %w", err)
		}
		filled++
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

	rows, err := s.pool.Query(ctx, "SELECT canonical_name FROM models")
	if err != nil {
		return 0, fmt.Errorf("查询模型失败: %w", err)
	}
	defer rows.Close()

	var names []string
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return 0, fmt.Errorf("扫描模型名失败: %w", err)
		}
		names = append(names, name)
	}
	if err := rows.Err(); err != nil {
		return 0, err
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

		if _, err := s.pool.Exec(ctx,
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
	rows, err := s.pool.Query(ctx,
		`SELECT canonical_name, input_price::float8, output_price::float8, cache_read_price::float8
		 FROM models WHERE input_price > 0 OR output_price > 0`)
	if err != nil {
		return nil, fmt.Errorf("查询模型价格失败: %w", err)
	}
	defer rows.Close()

	out := make(map[string]ModelPrice, 64)
	for rows.Next() {
		var name string
		var price ModelPrice
		if err := rows.Scan(&name, &price.Input, &price.Output, &price.CacheRead); err != nil {
			return nil, fmt.Errorf("扫描模型价格失败: %w", err)
		}
		out[name] = price
	}
	return out, rows.Err()
}

// 确保 json 被使用（OpenRouterModel 的标签需要它）。
var _ = json.Marshal
