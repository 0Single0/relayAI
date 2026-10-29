package store_test

import (
	"context"
	"encoding/json"
	"reflect"
	"testing"
	"time"

	"relayai/backend/internal/store"
)

func TestKeyBindingsSurviveReloadAndPartialUpdates(t *testing.T) {
	s, _ := testDB(t)
	ctx := context.Background()
	rpm, concurrency := 40, 8
	daily := int64(100000)
	expires := "2030-01-01T00:00:00Z"
	created, plaintext, err := s.CreateKey(ctx, store.KeyInput{
		Name: "binding regression", ChannelAllow: []string{"group-a"},
		ModelAllow: []string{"model-a", "model-b"}, RPMLimit: &rpm,
		DailyTokenLimit: &daily, MaxConcurrency: &concurrency, ExpiresAt: &expires,
	})
	if err != nil {
		t.Fatal(err)
	}
	check := func(key store.APIKey, channel string) {
		t.Helper()
		if !reflect.DeepEqual(key.ChannelAllow, []string{channel}) ||
			!reflect.DeepEqual(key.ModelAllow, []string{"model-a", "model-b"}) {
			t.Fatalf("bindings lost after read/update: channels=%v models=%v", key.ChannelAllow, key.ModelAllow)
		}
		if key.RPMLimit == nil || *key.RPMLimit != rpm || key.DailyTokenLimit == nil ||
			*key.DailyTokenLimit != daily || key.MaxConcurrency == nil || *key.MaxConcurrency != concurrency ||
			key.ExpiresAt == nil {
			t.Fatal("unrelated fields changed")
		}
		got, err := time.Parse(time.RFC3339, *key.ExpiresAt)
		want, _ := time.Parse(time.RFC3339, expires)
		if err != nil || !got.Equal(want) {
			t.Fatal("expiration changed")
		}
	}
	list, err := s.ListKeys(ctx)
	if err != nil || len(list) != 1 {
		t.Fatalf("list: count=%d error=%v", len(list), err)
	}
	check(list[0], "group-a")
	resolved, err := s.ResolveKey(ctx, plaintext)
	if err != nil {
		t.Fatal(err)
	}
	check(*resolved, "group-a")
	patch := func(body string) *store.APIKey {
		t.Helper()
		var input store.KeyInput
		if err := json.Unmarshal([]byte(body), &input); err != nil {
			t.Fatal(err)
		}
		key, err := s.UpdateKey(ctx, created.ID, input)
		if err != nil {
			t.Fatal(err)
		}
		return key
	}
	disabled := patch(`{"enabled":false}`)
	check(*disabled, "group-a")
	if disabled.Enabled {
		t.Fatal("disable was not saved")
	}
	changed := patch(`{"channelAllow":["group-b"]}`)
	check(*changed, "group-b")
	if changed.Enabled {
		t.Fatal("changing a channel re-enabled the key")
	}
	reset := patch(`{"modelAllow":[],"rpmLimit":null,"dailyTokenLimit":null,"maxConcurrency":null,"expiresAt":null}`)
	if !reflect.DeepEqual(reset.ChannelAllow, []string{"group-b"}) || len(reset.ModelAllow) != 0 ||
		reset.RPMLimit != nil || reset.DailyTokenLimit != nil || reset.MaxConcurrency != nil || reset.ExpiresAt != nil {
		t.Fatal("explicit resets did not preserve the channel")
	}
}
