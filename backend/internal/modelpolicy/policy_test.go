package modelpolicy

import "testing"

func TestRulesSupportFutureModels(t *testing.T) {
	for _, tc := range []struct {
		rule, model string
		want        bool
	}{
		{"deepseek*", "deepseek-v5", true}, {"qwen*", "Qwen4-Plus", true},
		{"*/qwen*", "Alibaba/Qwen4", true}, {"deepseek*", "qwen4", false},
		{"deepseek-v4", "deepseek-v5", false}, {"*a*b", "aab", true},
		{"ab*bc", "abc", false}, {"glm.5+?", "glm.5+?", true}, {"glm.5+?", "glmx555", false},
		{"*", "qwen4", true}, {"qwen*plus", "qwen4-plus-extra", false},
	} {
		if got := Matches(tc.rule, tc.model); got != tc.want {
			t.Errorf("%s / %s: got %v", tc.rule, tc.model, got)
		}
	}
	if !Allows(nil, "unknown-new-model") {
		t.Fatal("empty rules must allow all supported models")
	}
	if !Allows(Defaults("deepseek"), "deepseek-v5") || Allows(Defaults("deepseek"), "qwen4") {
		t.Fatal("vendor defaults leaked across families")
	}
	if !Allows(Defaults("alibaba"), "Qwen4") {
		t.Fatal("Qwen4 should not require a registry update")
	}
	if err := Validate([]string{"deepseek-v5", "qwen*"}); err != nil {
		t.Fatal(err)
	}
	for _, invalid := range []string{"", "deepseek v5", "qwen\n4"} {
		if Validate([]string{invalid}) == nil {
			t.Errorf("accepted invalid rule %q", invalid)
		}
	}
}
