package store

import (
	"encoding/json"
	"testing"
)

func TestAccountOverridePresence(t *testing.T) {
	for _, test := range []struct {
		raw     string
		present bool
		value   int
	}{
		{`{}`, false, 0},
		{`{"maxInflight":null,"priorityOverride":null,"weightOverride":null}`, true, 0},
		{`{"maxInflight":1111,"priorityOverride":1111,"weightOverride":1111}`, true, 1111},
	} {
		var input AccountInput
		if err := json.Unmarshal([]byte(test.raw), &input); err != nil {
			t.Fatal(err)
		}
		if input.MaxInflightSet != test.present || input.PriorityOverrideSet != test.present || input.WeightOverrideSet != test.present {
			t.Fatalf("incorrect presence: %+v", input)
		}
		for _, value := range []*int{input.MaxInflight, input.PriorityOverride, input.WeightOverride} {
			if test.value == 0 && value != nil {
				t.Fatal("null override must reset to default")
			}
			if test.value != 0 && (value == nil || *value != test.value) {
				t.Fatal("override value lost")
			}
		}
	}
}
