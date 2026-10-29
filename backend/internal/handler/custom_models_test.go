package handler

import (
	"encoding/json"
	"testing"
)

func TestCustomModelOptionsPreserveMetadata(t *testing.T) {
	raw := json.RawMessage(`{"realm":"cn","models":[{"id":"model-a","display_name":"Model A","context_length":1000000,"max_output_tokens":32000,"capabilities":{"vision":true,"reasoning":true},"reasoning_efforts":["off","high"],"reasoning_effort_map":{"off":"none","high":"max"}}]}`)
	out, err := customModelOptions(raw, []string{"model-a"})
	if err != nil {
		t.Fatal(err)
	}
	var options struct {
		Realm  string `json:"realm"`
		Models []struct {
			ID           string            `json:"id"`
			Name         string            `json:"display_name"`
			Context      int               `json:"context_length"`
			Output       int               `json:"max_output_tokens"`
			Capabilities map[string]bool   `json:"capabilities"`
			EffortMap    map[string]string `json:"reasoning_effort_map"`
		} `json:"models"`
	}
	if err := json.Unmarshal(out, &options); err != nil {
		t.Fatal(err)
	}
	model := options.Models[0]
	if options.Realm != "cn" || model.Name != "Model A" || model.Context != 1000000 || model.Output != 32000 || !model.Capabilities["vision"] || model.EffortMap["high"] != "max" {
		t.Fatalf("metadata was lost: %s", out)
	}
	for _, raw := range []json.RawMessage{nil, json.RawMessage(`{}`), json.RawMessage(`{"models":["model-a"]}`)} {
		if _, err := customModelOptions(raw, []string{"model-a"}); err != nil {
			t.Fatalf("legacy catalogs must remain usable: %v", err)
		}
	}
}

func TestCustomModelOptionsRejectInvalidMetadata(t *testing.T) {
	for _, raw := range []string{
		`{"models":[]}`, `{"models":[{"id":"other"}]}`, `{"models":[null]}`,
		`{"models":[{"id":"model-a","context_length":-1}]}`,
		`{"models":[{"id":"model-a","context_length":100.5}]}`,
		`{"models":[{"id":"model-a","context_length":100,"max_output_tokens":101}]}`,
		`{"models":[{"id":"model-a","reasoning_efforts":["high","high"]}]}`,
		`{"models":[{"id":"model-a","reasoning_efforts":["high"],"reasoning_effort_map":{"high":""}}]}`,
		`{"models":[{"id":"model-a","reasoning_effort_map":{"high":"max"}}]}`,
		`{"models":[{"id":"model-a","reasoning_default_effort":"high"}]}`,
	} {
		if _, err := customModelOptions(json.RawMessage(raw), []string{"model-a"}); err == nil {
			t.Errorf("accepted invalid metadata: %s", raw)
		}
	}
}
