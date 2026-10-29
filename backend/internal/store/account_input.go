package store

import "encoding/json"

func (input *AccountInput) UnmarshalJSON(data []byte) error {
	type plain AccountInput
	var decoded plain
	if err := json.Unmarshal(data, &decoded); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	*input = AccountInput(decoded)
	_, input.MaxInflightSet = fields["maxInflight"]
	_, input.PriorityOverrideSet = fields["priorityOverride"]
	_, input.WeightOverrideSet = fields["weightOverride"]
	return nil
}
