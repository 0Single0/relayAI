// Package modelpolicy defines group publication rules independently of pricing
// and vendor attribution. A rule never creates an account capability.
package modelpolicy

import (
	"fmt"
	"strings"
	"unicode"
)

// Matches is an anchored, case-insensitive glob. Only * is special;
// punctuation (including /, ., + and ?) is literal.
func Matches(rule, name string) bool {
	rule, name = strings.ToLower(rule), strings.ToLower(name)
	parts := strings.Split(rule, "*")
	if len(parts) == 1 {
		return rule == name
	}
	if !strings.HasPrefix(name, parts[0]) {
		return false
	}
	name = name[len(parts[0]):]
	for _, part := range parts[1 : len(parts)-1] {
		at := strings.Index(name, part)
		if at < 0 {
			return false
		}
		name = name[at+len(part):]
	}
	return strings.HasSuffix(name, parts[len(parts)-1])
}

func Allows(rules []string, name string) bool {
	if len(rules) == 0 {
		return true
	}
	for _, rule := range rules {
		if Matches(rule, name) {
			return true
		}
	}
	return false
}

func Validate(rules []string) error {
	if len(rules) > 200 {
		return fmt.Errorf("发布规则最多 200 条")
	}
	for _, rule := range rules {
		if rule == "" || len(rule) > 200 || strings.ContainsFunc(rule, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }) {
			return fmt.Errorf("模型名称或规则不能为空、包含空白字符或超过 200 字节")
		}
	}
	return nil
}

// Defaults are editable naming conventions, not proof of model ownership.
// Unknown vendors require explicit publication rules when creating a group.
func Defaults(provider string) []string {
	families := map[string][]string{
		"openai":    {"gpt*", "chatgpt*", "o1*", "o3*", "o4*"},
		"anthropic": {"claude*"}, "deepseek": {"deepseek*"},
		"google": {"gemini*", "gemma*"}, "alibaba": {"qwen*", "qwq*", "qvq*"},
		"zhipu": {"glm*", "chatglm*"}, "moonshot": {"kimi*", "moonshot*"},
		"minimax": {"minimax*"}, "tencent": {"hunyuan*", "hy-*"},
	}
	var rules []string
	for _, family := range families[provider] {
		rules = append(rules, family, "*/"+family)
	}
	return rules
}
