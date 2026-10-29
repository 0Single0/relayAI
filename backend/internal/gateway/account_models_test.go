package gateway

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"

	"relayai/backend/internal/config"
)

func TestAccountModelsFetchesEachTimeAndPreservesUnknownPrice(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/internal/accounts/wb-intl/user/name/models" || r.Header.Get("X-Internal-Token") != "internal-test" {
			t.Errorf("incorrect account or authentication: %s", r.URL.Path)
		}
		calls++
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintf(w, `{"realm":"intl","fetchedAtMs":%d,"models":[{"id":"new-model","credits":null,"creditMultiplier":null,"chargeType":"unknown"}]}`, calls)
	}))
	defer server.Close()
	client := New(config.GatewayConf{BaseURL: server.URL, InternalToken: "internal-test"})
	for call := int64(1); call <= 2; call++ {
		catalog, err := client.GetAccountModels(context.Background(), "wb-intl", "user/name")
		if err != nil {
			t.Fatal(err)
		}
		if catalog.Realm != "intl" || catalog.FetchedAtMs != call || len(catalog.Models) != 1 {
			t.Fatalf("unexpected catalog: %+v", catalog)
		}
		if catalog.Models[0].ChargeType != "unknown" || catalog.Models[0].CreditMultiplier != nil {
			t.Fatal("missing upstream price must stay unknown")
		}
	}
	if calls != 2 {
		t.Fatalf("expected two upstream requests, got %d", calls)
	}
}

func TestAccountModelsDoesNotFallbackAfterUpstreamFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusBadGateway)
		fmt.Fprint(w, `{"error":"account authorization failed","kind":"AuthExpired"}`)
	}))
	defer server.Close()
	client := New(config.GatewayConf{BaseURL: server.URL})
	result, err := client.GetAccountModels(context.Background(), "wb-cn", "account")
	var apiError *APIError
	if result != nil || !errors.As(err, &apiError) || apiError.Status != http.StatusBadGateway {
		t.Fatalf("expected an upstream failure without fallback, got %v, %v", result, err)
	}
}
