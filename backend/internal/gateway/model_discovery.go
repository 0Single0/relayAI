package gateway

import (
	"context"
	"errors"
	"net"
	"net/http"
)

type ModelDiscoveryInput struct {
	BaseURL   string `json:"baseUrl"`
	APIKey    string `json:"apiKey"`
	Protocol  string `json:"protocol"`
	AccountID *int64 `json:"accountId,omitempty"`
}

func (c *Client) DiscoverModels(ctx context.Context, input ModelDiscoveryInput) ([]UpstreamModel, error) {
	var out []UpstreamModel
	err := c.post(ctx, "/internal/model-discoveries", map[string]any{
		"base_url": input.BaseURL, "api_key": input.APIKey, "protocol": input.Protocol,
		"account_id": input.AccountID,
	}, &out)
	var upstreamError *APIError
	var connectionError *net.OpError
	var timeoutError net.Error
	if errors.As(err, &timeoutError) && timeoutError.Timeout() {
		return nil, &APIError{Status: http.StatusGatewayTimeout, Msg: "获取模型列表超时，请检查上游地址和服务后重试"}
	}
	if errors.As(err, &connectionError) {
		return nil, &APIError{Status: http.StatusBadGateway, Msg: "无法连接模型网关，请检查网关是否正常运行"}
	}
	if errors.As(err, &upstreamError) && upstreamError.Status == http.StatusNotFound {
		return nil, &APIError{Status: http.StatusBadGateway, Msg: "网关或上游未提供模型列表接口（HTTP 404），请确认网关已更新、上游地址正确，也可手动添加模型"}
	}
	if errors.As(err, &upstreamError) && (upstreamError.Status == http.StatusUnauthorized || upstreamError.Status == http.StatusForbidden) {
		// An upstream key failure must not invalidate the administrator's session.
		return nil, &APIError{Status: http.StatusBadGateway, Msg: "上游鉴权失败，请检查 API Key 和上游协议"}
	}
	return out, err
}
