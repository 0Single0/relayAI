package handler

import (
	"encoding/json"
	"testing"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"
)

func TestChannelHealthUsesAccountGroupsInsteadOfAdapterIDs(t *testing.T) {
	// Two WorkBuddy groups share one adapter. Include a mixed-platform group,
	// blocked/disabled accounts, and an empty group to avoid false online counts.
	var status gateway.Status
	err := json.Unmarshal([]byte(`{
		"running": true,
		"channels": [{"id":"workbuddy","adapter":"workbuddy","accounts":0,"ready":0}],
		"accounts": [
			{"channelId":"wb-cn","uid":"same-uid","platform":"workbuddy","enabled":true,"ready":true},
			{"channelId":"wb-cn","uid":"blocked","platform":"workbuddy","enabled":true,"ready":false},
			{"channelId":"wb-intl","uid":"same-uid","platform":"workbuddy","enabled":true,"ready":true},
			{"channelId":"wb-intl","uid":"second","platform":"workbuddy","enabled":true,"ready":true},
			{"channelId":"wb-intl","uid":"disabled","platform":"workbuddy","enabled":false,"ready":true},
			{"channelId":"mixed","uid":"one","platform":"workbuddy","enabled":true,"ready":true},
			{"channelId":"mixed","uid":"two","platform":"custom","enabled":true,"ready":true},
			{"channelId":"other","uid":"unrelated","platform":"workbuddy","enabled":true,"ready":true}
		]
	}`), &status)
	if err != nil {
		t.Fatal(err)
	}
	list := []store.ChannelHealth{
		{ID: "wb-cn", Enabled: true},
		{ID: "wb-intl", Enabled: true},
		{ID: "mixed", Enabled: true},
		{ID: "empty", Enabled: true},
	}
	attachChannelRuntime(list, &status)
	for position, want := range []int{1, 2, 2, 0} {
		got := list[position].ReadyAccounts
		if got == nil || *got != want {
			t.Errorf("group %s: ready accounts = %v, want %d", list[position].ID, got, want)
		}
	}
}

func TestChannelHealthOfflineDoesNotReuseRuntime(t *testing.T) {
	for _, status := range []*gateway.Status{nil, {Running: false, Accounts: []gateway.AccountRuntime{
		{ChannelID: "wb-cn", Enabled: true, Ready: true},
	}}} {
		oldCount := 7
		list := []store.ChannelHealth{{ID: "wb-cn", ReadyAccounts: &oldCount}}
		attachChannelRuntime(list, status)
		if list[0].ReadyAccounts != nil {
			t.Fatal("offline gateway must leave ready accounts unknown")
		}
	}
}
