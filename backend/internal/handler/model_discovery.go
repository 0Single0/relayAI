package handler

import (
	"net/http"
	"net/url"
	"strings"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"
)

func cleanModelIDs(values []string) []string {
	result := make([]string, 0, len(values))
	seen := map[string]bool{}
	for _, value := range values {
		value = strings.TrimSpace(value)
		if value != "" && !seen[value] {
			result = append(result, value)
			seen[value] = true
		}
	}
	return result
}

func registerModelDiscovery(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("POST /model-discoveries", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}
		var input gateway.ModelDiscoveryInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}
		input.BaseURL = strings.TrimSpace(input.BaseURL)
		input.APIKey = strings.TrimSpace(input.APIKey)
		parsed, err := url.Parse(input.BaseURL)
		if err != nil || parsed.Host == "" || (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.User != nil {
			httpx.Fail(w, http.StatusBadRequest, "请填写有效的 HTTP 或 HTTPS 上游地址")
			return
		}
		if input.AccountID != nil {
			account, err := ctx.Store.GetAccount(r.Context(), *input.AccountID)
			if err != nil {
				httpx.FailError(w, r, err)
				return
			}
			if account.Platform != "custom" {
				httpx.Fail(w, http.StatusBadRequest, "只有自定义供应商可以获取上游模型列表")
				return
			}
		}
		if input.APIKey == "" && input.AccountID == nil {
			httpx.Fail(w, http.StatusBadRequest, "请先填写 API Key")
			return
		}
		if input.Protocol != "openai-chat" && input.Protocol != "openai-responses" && input.Protocol != "anthropic" {
			httpx.Fail(w, http.StatusBadRequest, "不支持的上游协议")
			return
		}
		models, err := ctx.Gateway.DiscoverModels(r.Context(), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, models)
	})
}
