package store

import "strings"

// BuiltinPricesVerifiedAt is the date these official pages were fetched and checked.
const BuiltinPricesVerifiedAt = "2026-10-29"

const (
	openAIPriceSource   = "https://developers.openai.com/api/docs/pricing"
	claudePriceSource   = "https://platform.claude.com/docs/en/about-claude/pricing"
	geminiPriceSource   = "https://ai.google.dev/gemini-api/docs/pricing"
	deepSeekPriceSource = "https://api-docs.deepseek.com/quick_start/pricing/"
	bigModelPriceSource = "https://docs.bigmodel.cn/cn/guide/start/pricing"
	zAIPriceSource      = "https://docs.z.ai/guides/overview/pricing"
	kimiPriceSource     = "https://platform.kimi.com/docs/pricing/chat"
	miniMaxPriceSource  = "https://platform.minimax.io/docs/pricing/overview"
	tokenHubPriceSource = "https://cloud.tencent.com/document/product/1823/130055"
	qwenPriceSource     = "https://help.aliyun.com/zh/model-studio/model-pricing"
)

// BuiltinPrice retains the published currency and pricing scope. ModelPrice on
// its own is used for CNY database/runtime prices; these reference quotes can be
// USD or CNY. Basis documents the selected tier, not a full tiered tariff.
type BuiltinPrice struct {
	ModelPrice
	Currency  string
	SourceURL string
	Basis     string
}

func usdPrice(input, output, cache float64, source, basis string) BuiltinPrice {
	return BuiltinPrice{ModelPrice{input, output, cache}, "USD", source, basis}
}

func cnyPrice(input, output, cache float64, source, basis string) BuiltinPrice {
	return BuiltinPrice{ModelPrice{input, output, cache}, "CNY", source, basis}
}

func (price BuiltinPrice) inCNY(usdToCny float64) ModelPrice {
	if price.Currency == "CNY" {
		return price.ModelPrice
	}
	return ModelPrice{price.Input * usdToCny, price.Output * usdToCny, price.CacheRead * usdToCny}
}

// BuiltinPrices contains official public reference rates per million tokens.
// It is an offline snapshot, not WorkBuddy credit pricing or a live price feed.
// Only explicitly listed model IDs match. See docs/builtin-model-prices.md for
// sources, unsupported IDs, and limitations of the single-rate database schema.
var BuiltinPrices = map[string]BuiltinPrice{
	// OpenAI: Standard, short context; no Batch/Flex/Fast or regional uplift.
	"gpt-6-astra":            usdPrice(10, 50, 1, openAIPriceSource, "Standard, short context"),
	"gpt-6.1-sol":            usdPrice(2, 10, 0.1, openAIPriceSource, "Standard, short context"),
	"gpt-6.1-terra":          usdPrice(1, 5, 0.05, openAIPriceSource, "Standard, short context"),
	"gpt-6-luna":             usdPrice(0.1, 0.5, 0.01, openAIPriceSource, "Standard, short context"),
	"gpt-6-sol":              usdPrice(2, 10, 0.2, openAIPriceSource, "Standard, short context"),
	"gpt-5.5":                usdPrice(5, 30, 0.5, openAIPriceSource, "Standard, context <272K"),
	"gpt-5.5-pro":            usdPrice(30, 180, 0, openAIPriceSource, "Standard, context <272K; cache not quoted"),
	"gpt-5.4":                usdPrice(2.5, 15, 0.25, openAIPriceSource, "Standard, context <272K"),
	"gpt-5.4-pro":            usdPrice(30, 180, 0, openAIPriceSource, "Standard, context <272K; cache not quoted"),
	"gpt-5.4-mini":           usdPrice(0.75, 4.5, 0.075, openAIPriceSource, "Standard"),
	"gpt-5.4-nano":           usdPrice(0.2, 1.25, 0.02, openAIPriceSource, "Standard"),
	"gpt-5.2":                usdPrice(1.75, 14, 0.175, openAIPriceSource, "Standard"),
	"gpt-5.2-pro":            usdPrice(21, 168, 0, openAIPriceSource, "Standard; cache not quoted"),
	"gpt-5.1":                usdPrice(1.25, 10, 0.125, openAIPriceSource, "Standard"),
	"gpt-5":                  usdPrice(1.25, 10, 0.125, openAIPriceSource, "Standard"),
	"gpt-5-mini":             usdPrice(0.25, 2, 0.025, openAIPriceSource, "Standard"),
	"gpt-5-nano":             usdPrice(0.05, 0.4, 0.005, openAIPriceSource, "Standard"),
	"gpt-5-pro":              usdPrice(15, 120, 0, openAIPriceSource, "Standard; cache not quoted"),
	"gpt-4.1":                usdPrice(2, 8, 0.5, openAIPriceSource, "Standard"),
	"gpt-4.1-mini":           usdPrice(0.4, 1.6, 0.1, openAIPriceSource, "Standard"),
	"gpt-4.1-nano":           usdPrice(0.1, 0.4, 0.025, openAIPriceSource, "Standard"),
	"gpt-4o":                 usdPrice(2.5, 10, 1.25, openAIPriceSource, "Standard"),
	"gpt-4o-2024-05-13":      usdPrice(5, 15, 0, openAIPriceSource, "Standard; distinct legacy price; cache not quoted"),
	"gpt-4o-mini":            usdPrice(0.15, 0.6, 0.075, openAIPriceSource, "Standard"),
	"o1":                     usdPrice(15, 60, 7.5, openAIPriceSource, "Standard"),
	"o1-pro":                 usdPrice(150, 600, 0, openAIPriceSource, "Standard; cache not quoted"),
	"o3-pro":                 usdPrice(20, 80, 0, openAIPriceSource, "Standard; cache not quoted"),
	"o3":                     usdPrice(2, 8, 0.5, openAIPriceSource, "Standard"),
	"o3-mini":                usdPrice(1.1, 4.4, 0.55, openAIPriceSource, "Standard"),
	"o4-mini":                usdPrice(1.1, 4.4, 0.275, openAIPriceSource, "Standard"),
	"gpt-4-turbo-2024-04-09": usdPrice(10, 30, 0, openAIPriceSource, "Standard; cache not quoted"),
	"gpt-4-0613":             usdPrice(30, 60, 0, openAIPriceSource, "Standard; cache not quoted"),
	"gpt-3.5-turbo":          usdPrice(0.5, 1.5, 0, openAIPriceSource, "Standard; cache not quoted"),
	"gpt-3.5-turbo-0125":     usdPrice(0.5, 1.5, 0, openAIPriceSource, "Standard; cache not quoted"),

	// Claude: base input/output and cache hits, not cache writes or Fast mode.
	"claude-fable-5-1":  usdPrice(10, 50, 0.25, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-5-5":   usdPrice(4, 20, 0.2, claudePriceSource, "Base tokens, cache hits"),
	"claude-sonnet-5-5": usdPrice(2, 10, 0.1, claudePriceSource, "Base tokens, cache hits"),
	"claude-haiku-5-5":  usdPrice(0.1, 0.5, 0.01, claudePriceSource, "Prompts <=100K"),
	"claude-mythos-5-1": usdPrice(10, 50, 0.25, claudePriceSource, "Base tokens, cache hits"),
	"claude-fable-5":    usdPrice(10, 50, 1, claudePriceSource, "Base tokens, cache hits"),
	"claude-mythos-5":   usdPrice(10, 50, 1, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-5":     usdPrice(5, 25, 0.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4-8":   usdPrice(5, 25, 0.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4-7":   usdPrice(5, 25, 0.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4-6":   usdPrice(5, 25, 0.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4-5":   usdPrice(5, 25, 0.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4-1":   usdPrice(15, 75, 1.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-opus-4":     usdPrice(15, 75, 1.5, claudePriceSource, "Base tokens, cache hits"),
	"claude-sonnet-5":   usdPrice(2, 10, 0.2, claudePriceSource, "Base tokens, cache hits"),
	"claude-sonnet-4-6": usdPrice(3, 15, 0.3, claudePriceSource, "Base tokens, cache hits"),
	"claude-sonnet-4-5": usdPrice(3, 15, 0.3, claudePriceSource, "Base tokens, cache hits"),
	"claude-sonnet-4":   usdPrice(3, 15, 0.3, claudePriceSource, "Base tokens, cache hits"),
	"claude-haiku-4-5":  usdPrice(1, 5, 0.1, claudePriceSource, "Base tokens, cache hits"),
	"claude-3-5-haiku":  usdPrice(0.8, 4, 0.08, claudePriceSource, "Base tokens, cache hits"),

	// Gemini: standard paid text input; no audio, grounding, storage or Batch.
	"gemini-2.5-pro":         usdPrice(1.25, 10, 0.125, geminiPriceSource, "Paid Standard, prompts <=200K"),
	"gemini-2.5-flash":       usdPrice(0.3, 2.5, 0.03, geminiPriceSource, "Paid Standard, text/image/video input"),
	"gemini-2.5-flash-lite":  usdPrice(0.1, 0.4, 0.01, geminiPriceSource, "Paid Standard, text/image/video input"),
	"gemini-3.1-pro-preview": usdPrice(2, 12, 0.2, geminiPriceSource, "Paid Standard, prompts <=200K"),
	"gemini-3.5-flash-lite":  usdPrice(0.3, 2.5, 0.03, geminiPriceSource, "Paid Standard"),

	// DeepSeek: peak rate, not the 50%-off off-peak rate. The official page
	// explicitly says legacy v4-flash now uses V4.1-Flash-1017 and Flash pricing.
	"deepseek-flash":           usdPrice(0.3, 1.2, 0.006, deepSeekPriceSource, "Peak; V4.1-Flash-1017"),
	"deepseek-v4-flash":        usdPrice(0.3, 1.2, 0.006, deepSeekPriceSource, "Peak; officially redirected to V4.1-Flash-1017"),
	"deepseek-v4.1-flash":      usdPrice(0.3, 1.2, 0.006, deepSeekPriceSource, "Peak; V4.1-Flash-1017 reference"),
	"deepseek-v4.1-flash-1017": usdPrice(0.3, 1.2, 0.006, deepSeekPriceSource, "Peak; V4.1-Flash-1017"),
	"deepseek-v4-pro":          usdPrice(1.32, 3.96, 0.044, deepSeekPriceSource, "Peak; V4-Pro-0813"),

	// GLM: BigModel CN prices where available; shorter-input tier is explicit.
	"glm-5.3":              cnyPrice(8, 28, 2, bigModelPriceSource, "BigModel CN"),
	"glm-5.3-flash":        cnyPrice(0.8, 2.8, 0.23, bigModelPriceSource, "BigModel CN"),
	"glm-5.3-flashx":       cnyPrice(2, 7, 0.57, bigModelPriceSource, "BigModel CN"),
	"glm-5.3-flashx-legal": cnyPrice(2, 7, 0.57, bigModelPriceSource, "BigModel CN"),
	"glm-5.2":              cnyPrice(8, 28, 2, bigModelPriceSource, "BigModel CN"),
	"glm-5.1":              cnyPrice(6, 24, 1.3, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-5-turbo":          cnyPrice(5, 22, 1.2, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-5":                cnyPrice(4, 18, 1, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-5v-turbo":         cnyPrice(5, 22, 1.2, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-4.7":              usdPrice(0.6, 2.2, 0.11, zAIPriceSource, "Z.AI international"),
	"glm-4.6":              usdPrice(0.6, 2.2, 0.11, zAIPriceSource, "Z.AI international"),
	"glm-4.5":              usdPrice(0.6, 2.2, 0.11, zAIPriceSource, "Z.AI international"),
	"glm-4.5-x":            usdPrice(2.2, 8.9, 0.45, zAIPriceSource, "Z.AI international"),
	"glm-4.5-air":          usdPrice(0.2, 1.1, 0.03, zAIPriceSource, "Z.AI international"),
	"glm-4.5-airx":         usdPrice(1.1, 4.5, 0.22, zAIPriceSource, "Z.AI international"),
	"glm-4.7-flashx":       usdPrice(0.07, 0.4, 0.01, zAIPriceSource, "Z.AI international"),
	"glm-4.6v":             cnyPrice(1, 3, 0.2, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-4.6v-flashx":      cnyPrice(0.15, 1.5, 0.03, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-4.5v":             cnyPrice(2, 6, 0.4, bigModelPriceSource, "BigModel CN, input <32K"),
	"glm-ocr":              cnyPrice(0.2, 0.2, 0, bigModelPriceSource, "BigModel CN; cache not supported"),

	// Kimi: the manufacturer's CN pricing, not the international tariff.
	"kimi-k3.1-pro":            cnyPrice(13, 68, 1.3, kimiPriceSource, "Kimi CN; cache write priced separately"),
	"kimi-k3.1-pro-highspeed":  cnyPrice(26, 136, 1.3, kimiPriceSource, "Kimi CN Highspeed; cache write priced separately"),
	"kimi-k3":                  cnyPrice(20, 100, 2, kimiPriceSource, "Kimi CN; cache write priced separately"),
	"kimi-k2.7-code":           cnyPrice(6.5, 27, 1.3, kimiPriceSource, "Kimi CN"),
	"kimi-k2.7-code-highspeed": cnyPrice(13, 54, 2.6, kimiPriceSource, "Kimi CN Highspeed"),
	"kimi-k2.6":                cnyPrice(6.5, 27, 1.1, kimiPriceSource, "Kimi CN"),

	// MiniMax: Standard service; published permanent 50% reduction for M3.
	"minimax-m3":         usdPrice(0.3, 1.2, 0.06, miniMaxPriceSource, "Standard, input <=512K, permanent 50% off"),
	"minimax-m3.1-flash": usdPrice(0.15, 0.6, 0.015, miniMaxPriceSource, "Standard"),

	// Tencent's own Hy models. Other entries below are explicitly TokenHub's
	// hosted quotes, not claims about the original model manufacturer's price.
	"hy3":               cnyPrice(1, 4, 0.25, tokenHubPriceSource, "Tencent TokenHub Guangzhou, Hy3"),
	"hy4-preview":       cnyPrice(6, 18, 0.3, tokenHubPriceSource, "Tencent TokenHub Guangzhou, Hy4 preview"),
	"kimi-k2.8-preview": cnyPrice(6.5, 27, 1.7, tokenHubPriceSource, "TokenHub Guangzhou hosted reference; not manufacturer tariff"),
	"minimax-m2.7":      cnyPrice(2.1, 8.4, 0.42, tokenHubPriceSource, "TokenHub Guangzhou hosted reference; not manufacturer tariff"),
	"qwen3.5-flash":     cnyPrice(0.2, 2, 0.02, tokenHubPriceSource, "TokenHub Guangzhou hosted reference, input <=128K"),
	"qwen3.5-plus":      cnyPrice(0.8, 4.8, 0.08, tokenHubPriceSource, "TokenHub Guangzhou hosted reference, input <=128K"),

	// Alibaba Model Studio CN Beijing: listed base prices, no limited-time
	// discount. Cache not quoted here => 0, which runtime bills at input price.
	"qwen3.7-plus":            cnyPrice(2, 8, 0, qwenPriceSource, "CN Beijing, input <=256K, list price, no assumed cache discount"),
	"qwen3.7-plus-2026-05-26": cnyPrice(2, 8, 0, qwenPriceSource, "CN Beijing, input <=256K, no assumed cache discount"),
	"qwen3.6-plus":            cnyPrice(2, 12, 0, qwenPriceSource, "CN Beijing, input <=256K, no assumed cache discount"),
	"qwen3.6-plus-2026-04-02": cnyPrice(2, 12, 0, qwenPriceSource, "CN Beijing, input <=256K, no assumed cache discount"),
	"qwen-plus":               cnyPrice(0.8, 2, 0, qwenPriceSource, "CN Beijing, non-thinking, input <=128K, no assumed cache discount"),
	"qwen-plus-2025-12-01":    cnyPrice(0.8, 2, 0, qwenPriceSource, "CN Beijing, non-thinking, input <=128K, no assumed cache discount"),
}

// Never infer pricing from a prefix: gpt-5-pro, GLM Flash, dated snapshots and
// hosted route variants can all cost differently from their apparent base ID.
func lookupBuiltinPrice(model string) (BuiltinPrice, bool) {
	price, ok := BuiltinPrices[strings.ToLower(strings.TrimSpace(model))]
	return price, ok
}
