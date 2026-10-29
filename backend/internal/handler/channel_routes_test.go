package handler

import (
	"encoding/json"
	"testing"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"
)

func TestChannelRoutesUseGroupAccountModels(t *testing.T) {
	var catalog []gateway.CatalogModel
	if err := json.Unmarshal([]byte(`[
		{"name":"model-a","channels":[{"channelId":"first","upstreamModel":"model-a","enabled":true},{"channelId":"empty","enabled":true}]},
		{"name":"model-b","channels":[{"channelId":"second","upstreamModel":"model-b","enabled":true},{"channelId":"disabled","enabled":true}]}
	]`), &catalog); err != nil {
		t.Fatal(err)
	}
	channels := []store.Channel{
		{ID: "first", Enabled: true, AccountCount: 1},
		{ID: "second", Enabled: true, AccountCount: 1},
		{ID: "empty", Enabled: true},
		{ID: "disabled", AccountCount: 1},
	}
	routes := channelRouteIndex(channels, catalog)
	if len(routes["model-a"]) != 1 || routes["model-a"][0] != "first" {
		t.Fatalf("incorrect model-a providers: %v", routes)
	}
	if len(routes["model-b"]) != 1 || routes["model-b"][0] != "second" {
		t.Fatalf("incorrect model-b providers: %v", routes)
	}
}

func TestVisibleModelsRequireActualProviders(t *testing.T) {
	models := []store.Model{{CanonicalName: "stale", Enabled: true}}
	channels := []store.Channel{{ID: "empty", Enabled: true}}
	if got := visibleModels(channels, models, nil); len(got) != 0 {
		t.Fatalf("historical catalog leaked without account support: %v", got)
	}
}
