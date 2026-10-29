package handler

import (
	"encoding/json"
	"testing"

	"relayai/backend/internal/store"
)

func TestValidateAccountUpdate(t *testing.T) {
	account := store.Account{Platform: "custom", ModelAllow: []string{"old"}, Options: json.RawMessage(`{"models":["old"],"provider_setting":"keep"}`)}
	for _, test := range []struct {
		name, raw string
		invalid   bool
	}{
		{"models and settings", `{"maxInflight":1111,"baseUrl":"https://example.test/v1","protocol":"openai-responses","modelAllow":["new"],"options":{"models":[{"id":"new","context_length":1000000,"max_output_tokens":32000}]}}`, false},
		{"reset default", `{"maxInflight":null,"priorityOverride":null,"weightOverride":null}`, false},
		{"zero concurrency", `{"maxInflight":0}`, true},
		{"negative concurrency", `{"maxInflight":-2}`, true},
		{"overflow", `{"maxInflight":2147483648}`, true},
		{"invalid url", `{"baseUrl":"ftp://example.test"}`, true},
		{"url credentials", `{"baseUrl":"https://user:secret@example.test"}`, true},
		{"unknown protocol", `{"protocol":"unknown"}`, true},
		{"empty models", `{"modelAllow":[],"options":{"models":[]}}`, true},
		{"mismatched models", `{"modelAllow":["other"],"options":{"models":["new"]}}`, true},
		{"bad alias", `{"modelRename":{"model":""}}`, true},
	} {
		t.Run(test.name, func(t *testing.T) {
			var input store.AccountInput
			if err := json.Unmarshal([]byte(test.raw), &input); err != nil {
				if test.invalid {
					return
				}
				t.Fatal(err)
			}
			err := validateAccountUpdate(account, &input)
			if (err != nil) != test.invalid {
				t.Fatalf("validation error: %v", err)
			}
			if !test.invalid && input.Options != nil {
				var options map[string]any
				_ = json.Unmarshal(input.Options, &options)
				if options["provider_setting"] != "keep" {
					t.Fatal("unrelated supplier setting lost")
				}
			}
		})
	}
}

func TestOAuthUpdateKeepsPlatformAndRealm(t *testing.T) {
	account := store.Account{Platform: "workbuddy", Options: json.RawMessage(`{"realm":"cn"}`)}
	for _, raw := range []string{`{"apiKey":"key"}`, `{"options":{"realm":"intl"}}`, `{"options":{"outbound_identity":"unknown"}}`, `{"baseUrl":"https://example.test"}`} {
		var input store.AccountInput
		_ = json.Unmarshal([]byte(raw), &input)
		if validateAccountUpdate(account, &input) == nil {
			t.Fatalf("invalid OAuth update accepted: %s", raw)
		}
	}
	var input store.AccountInput
	_ = json.Unmarshal([]byte(`{"options":{"outbound_identity":"cli"}}`), &input)
	if err := validateAccountUpdate(account, &input); err != nil {
		t.Fatal(err)
	}
	var options map[string]string
	_ = json.Unmarshal(input.Options, &options)
	if options["realm"] != "cn" {
		t.Fatal("realm lost")
	}
}
