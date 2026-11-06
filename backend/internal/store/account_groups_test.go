package store

import "testing"

func TestNormalizeAccountGroups(t *testing.T) {
	groups, err := normalizeAccountGroups([]AccountGroupInput{
		{GroupID: " deepseek-standard ", Enabled: true},
		{GroupID: "openai-standard", Enabled: true, Priority: intPtr(120), Weight: intPtr(3)},
	})
	if err != nil || len(groups) != 2 || groups[0].GroupID != "deepseek-standard" {
		t.Fatalf("unexpected normalized groups: %#v, %v", groups, err)
	}
	if _, err := normalizeAccountGroups([]AccountGroupInput{{GroupID: "same"}, {GroupID: "same"}}); err == nil {
		t.Fatal("duplicate membership should be rejected")
	}
}

func TestBuiltinModelProviderUsesCuratedEvidence(t *testing.T) {
	for name, want := range map[string]string{
		"gpt-4o": "openai",
		"deepseek-v4-flash": "deepseek",
		"glm-5.3": "zhipu",
		"totally-unknown": "",
	} {
		if got := builtinModelProvider(name); got != want {
			t.Errorf("builtinModelProvider(%q) = %q, want %q", name, got, want)
		}
	}
}

func intPtr(value int) *int { return &value }
