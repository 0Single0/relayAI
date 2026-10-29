package store

import (
	"encoding/json"
	"testing"
)

func TestAccountMatchesChannel(t *testing.T) {
	cn := json.RawMessage(`{"realm":"cn"}`)
	intl := json.RawMessage(`{"realm":"intl"}`)
	for _, test := range []struct {
		name    string
		account Account
		channel Channel
		want    bool
	}{
		{"same platform", Account{Platform: "workbuddy", Options: cn}, Channel{Adapter: "workbuddy", Options: cn}, true},
		{"cross platform", Account{Platform: "workbuddy", Options: cn}, Channel{Adapter: "trae"}, false},
		{"cross region", Account{Platform: "workbuddy", Options: cn}, Channel{Adapter: "workbuddy", Options: intl}, false},
		{"international", Account{Platform: "workbuddy", Options: intl}, Channel{Adapter: "workbuddy", Options: intl}, true},
		{"legacy platform", Account{Adapter: "workbuddy"}, Channel{Adapter: "workbuddy", Options: cn}, true},
		{"custom", Account{Platform: "custom"}, Channel{Adapter: "custom"}, true},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := AccountMatchesChannel(test.account, test.channel); got != test.want {
				t.Fatalf("got %v, want %v", got, test.want)
			}
		})
	}
}
