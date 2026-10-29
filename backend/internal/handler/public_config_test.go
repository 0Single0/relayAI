package handler

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"relayai/backend/internal/config"
	"relayai/backend/internal/gateway"
	"relayai/backend/internal/svc"
)

func TestPublicConfigGatewayAddress(t *testing.T) {
	for _, test := range []struct {
		name, public, want string
		sameOrigin         bool
	}{
		{"docker", "same-origin", "", true},
		{"development", "", "http://127.0.0.1:18788", false},
		{"separate gateway", "https://api.example.test/", "https://api.example.test", false},
	} {
		t.Run(test.name, func(t *testing.T) {
			conf := config.GatewayConf{BaseURL: "http://127.0.0.1:18788", PublicBaseURL: test.public}
			ctx := &svc.ServiceContext{Gateway: gateway.New(conf)}
			mux := http.NewServeMux()
			registerHealth(mux, ctx)
			request := httptest.NewRequest(http.MethodGet, "http://admin.example.test/public-config", nil)
			request.Header.Set("X-Forwarded-Host", "untrusted.example.test")
			response := httptest.NewRecorder()
			mux.ServeHTTP(response, request)
			var body struct {
				Success bool `json:"success"`
				Data    struct {
					BaseURL    string `json:"gatewayBaseUrl"`
					SameOrigin bool   `json:"gatewaySameOrigin"`
				} `json:"data"`
			}
			if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
				t.Fatal(err)
			}
			if !body.Success || body.Data.BaseURL != test.want || body.Data.SameOrigin != test.sameOrigin {
				t.Fatalf("unexpected public configuration: %s", response.Body.String())
			}
		})
	}
}
