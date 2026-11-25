package handler

import (
	"encoding/json"
	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"
	"testing"
)

func TestProbeUsesActualAccountSupportAndPublication(t *testing.T) {
	var catalog []gateway.CatalogModel
	if err := json.Unmarshal([]byte(`[
	 {"name":"deepseek-v5","channels":[{"channelId":"ds","enabled":true,"accountUids":["supports-ds"]}]},
	 {"name":"qwen4","channels":[{"channelId":"ds","enabled":false,"accountUids":["supports-qwen"]}]}
	]`), &catalog); err != nil {
		t.Fatal(err)
	}
	targets := []store.ProbeTarget{
		{ChannelID: "ds", AccountUID: "wrong-primary-account", Model: "deepseek-v5", OutboundModel: "deepseek-v5"},
		{ChannelID: "ds", Model: "qwen4", OutboundModel: "qwen4"},
		{ChannelID: "ds", Model: "deepseek*", OutboundModel: "deepseek*"},
	}
	got := resolveProbeTargets(targets, catalog)
	if len(got) != 1 || got[0].AccountUID != "supports-ds" {
		t.Fatalf("unexpected targets: %+v", got)
	}
	if len(resolveProbeTargets(targets, nil)) != 0 {
		t.Fatal("historical config cannot imply live support")
	}
}
