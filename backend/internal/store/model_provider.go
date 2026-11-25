package store

import (
	"context"
	"regexp"
	"strings"

	"relayai/backend/internal/models"
)

var providerID = regexp.MustCompile(`^[a-z][a-z0-9-]{0,49}$`)

func ValidModelProvider(value string) bool { return providerID.MatchString(value) }

// Reuse only exact entries in the existing curated catalog. Unknown IDs stay
// unclassified; protocol, endpoint, and model-name prefixes are not evidence.
func builtinModelProvider(name string) string {
	switch name {
	case "kimi-k2.8-preview":
		return "moonshot"
	case "minimax-m2.7":
		return "minimax"
	case "qwen3.5-flash", "qwen3.5-plus":
		return "alibaba"
	}
	price, ok := lookupBuiltinPrice(name)
	if !ok {
		return ""
	}
	switch price.SourceURL {
	case openAIPriceSource:
		return "openai"
	case claudePriceSource:
		return "anthropic"
	case geminiPriceSource:
		return "google"
	case deepSeekPriceSource:
		return "deepseek"
	case bigModelPriceSource, zAIPriceSource:
		return "zhipu"
	case kimiPriceSource:
		return "moonshot"
	case miniMaxPriceSource:
		return "minimax"
	case qwenPriceSource:
		return "alibaba"
	case tokenHubPriceSource:
		return "tencent"
	}
	return ""
}

func (s *Store) ApplyBuiltinModelProviders(ctx context.Context) error {
	var names []string
	if err := s.db.WithContext(ctx).Model(&models.Models{}).Where("model_provider = ''").Pluck("canonical_name", &names).Error; err != nil {
		return err
	}
	for _, name := range names {
		if provider := builtinModelProvider(name); provider != "" {
			if err := s.db.WithContext(ctx).Model(&models.Models{}).Where("canonical_name = ? AND model_provider = ''", name).Update("model_provider", provider).Error; err != nil {
				return err
			}
		}
	}
	return nil
}

func normalizeProvider(value string) string { return strings.ToLower(strings.TrimSpace(value)) }
