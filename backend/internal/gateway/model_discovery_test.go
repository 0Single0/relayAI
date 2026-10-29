package gateway

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"testing"

	"relayai/backend/internal/config"
)

func TestModelDiscoveryProxy(t *testing.T) {
	status := http.StatusOK
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/internal/model-discoveries" {
			t.Errorf("unexpected request %s %s", r.Method, r.URL.Path)
		}
		var input map[string]string
		if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
			t.Error(err)
		}
		if input["base_url"] != "https://example.test/v1" || input["api_key"] != "test-key" || input["protocol"] != "openai-chat" {
			t.Errorf("incorrect discovery payload")
		}
		w.WriteHeader(status)
		if status == http.StatusOK {
			_, _ = w.Write([]byte(`[{"id":"model-a","display_name":"Model A","context_length":1000000,"max_output_tokens":32000,"capabilities":{"vision":true,"reasoning":true},"reasoning_efforts":["high"],"reasoning_effort_map":{"high":"max"}}]`))
		} else {
			_, _ = w.Write([]byte(`{"error":"invalid upstream key"}`))
		}
	}))
	defer server.Close()
	client := New(config.GatewayConf{BaseURL: server.URL})
	input := ModelDiscoveryInput{BaseURL: "https://example.test/v1", APIKey: "test-key", Protocol: "openai-chat"}
	models, err := client.DiscoverModels(context.Background(), input)
	if err != nil || len(models) != 1 || models[0].ID != "model-a" {
		t.Fatalf("unexpected response: %v %v", models, err)
	}
	if models[0].DisplayName != "Model A" || models[0].ContextLength == nil || *models[0].ContextLength != 1000000 || !models[0].Capabilities.Vision || models[0].ReasoningEffortMap["high"] != "max" {
		t.Fatalf("discovery metadata was lost: %+v", models[0])
	}
	for _, code := range []int{http.StatusUnauthorized, http.StatusForbidden} {
		status = code
		_, err := client.DiscoverModels(context.Background(), input)
		var apiError *APIError
		if !errors.As(err, &apiError) || apiError.Status != http.StatusBadGateway {
			t.Fatalf("upstream auth error must not become a session error: %v", err)
		}
	}
}
