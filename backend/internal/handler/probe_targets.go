package handler

import (
	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"
)

// The matrix knows which enabled membership actually supports a concrete model.
// A legacy primary channel or an unrestricted account allowlist is insufficient.
func resolveProbeTargets(targets []store.ProbeTarget, catalog []gateway.CatalogModel) []store.ProbeTarget {
	type routeKey struct{ group, model string }
	accounts := map[routeKey]string{}
	for _, model := range catalog {
		for _, route := range model.Channels {
			if route.Enabled && len(route.AccountUIDs) > 0 {
				accounts[routeKey{route.ChannelID, model.Name}] = route.AccountUIDs[0]
			}
		}
	}
	out := make([]store.ProbeTarget, 0, len(targets))
	for _, target := range targets {
		if uid := accounts[routeKey{target.ChannelID, target.OutboundModel}]; uid != "" {
			target.AccountUID = uid
			out = append(out, target)
		}
	}
	return out
}
