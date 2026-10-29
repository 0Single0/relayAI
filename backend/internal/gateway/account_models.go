package gateway

import "context"

type LiveAccountModels struct {
	Realm       string             `json:"realm"`
	FetchedAtMs int64              `json:"fetchedAtMs"`
	Models      []LiveAccountModel `json:"models"`
}

type LiveAccountModel struct {
	ID               string   `json:"id"`
	DisplayName      *string  `json:"displayName"`
	ContextWindow    *int64   `json:"contextWindow"`
	MaxOutput        *int64   `json:"maxOutput"`
	Credits          *string  `json:"credits"`
	CreditMultiplier *float64 `json:"creditMultiplier"`
	ChargeType       string   `json:"chargeType"`
	Capabilities     struct {
		Tools     bool `json:"tools"`
		Vision    bool `json:"vision"`
		Reasoning bool `json:"reasoning"`
	} `json:"capabilities"`
}

// GetAccountModels requests the selected account's current WorkBuddy catalog.
// There is intentionally no matrix, database, snapshot, or TTL fallback.
func (c *Client) GetAccountModels(ctx context.Context, channelID, uid string) (*LiveAccountModels, error) {
	var out LiveAccountModels
	if err := c.get(ctx, "/internal/accounts/"+AccountKey(channelID, uid)+"/models", &out); err != nil {
		return nil, err
	}
	return &out, nil
}
