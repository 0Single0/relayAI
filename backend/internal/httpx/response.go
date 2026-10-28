// Package httpx 提供统一的 JSON 响应、错误信封与参数解析。
//
// 响应信封（SPEC 7.1）：`{ code, success, msg?, data? }`，`0` 与 `200` 为成功。
package httpx

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strconv"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/store"

	"github.com/zeromicro/go-zero/core/logx"
)

// Envelope 是统一响应体。
type Envelope struct {
	Code    int    `json:"code"`
	Success bool   `json:"success"`
	Msg     string `json:"msg,omitempty"`
	Data    any    `json:"data,omitempty"`
}

// OK 返回成功响应。
func OK(w http.ResponseWriter, data any) {
	write(w, http.StatusOK, Envelope{Code: 0, Success: true, Data: data})
}

// OKMessage 返回只带消息的成功响应。
func OKMessage(w http.ResponseWriter, msg string) {
	write(w, http.StatusOK, Envelope{Code: 0, Success: true, Msg: msg})
}

// Fail 返回错误响应。
//
// 状态码与信封 code 保持一致：前端拦截器按 `code !== 0 && code !== 200` 判失败，
// 同时也能从 HTTP 状态码拿到语义。
func Fail(w http.ResponseWriter, status int, msg string) {
	write(w, status, Envelope{Code: status, Success: false, Msg: msg})
}

// FailError 把内部错误翻译成响应。
//
// 五类要分清（SPEC 6.4）：
//   - store.ErrNotFound          → 404
//   - store.ErrNotRunning        → 409（记录在，但状态不允许这个操作）
//   - store.ErrInsufficientBalance → 400（请求不成立，不是服务故障）
//   - store.ErrCodeInvalid       → 400（兑换码用不了）
//   - gateway.APIError           → 透传网关的状态码（403 不能变成 500）
//   - 其它                        → 500，且日志里保留原文
func FailError(w http.ResponseWriter, r *http.Request, err error) {
	if errors.Is(err, store.ErrNotFound) {
		Fail(w, http.StatusNotFound, "记录不存在")
		return
	}

	// 状态冲突用 409：前端据此提示"已经结束，不需要取消"，
	// 而不是把用户引去排查"记录是不是不存在"。
	if errors.Is(err, store.ErrNotRunning) {
		Fail(w, http.StatusConflict, err.Error())
		return
	}

	// 余额不足是**请求不成立**，不是服务故障：必须 400。
	// 报成 500 会让用户以为系统坏了，实际是他填的金额不对。
	if errors.Is(err, store.ErrInsufficientBalance) {
		Fail(w, http.StatusBadRequest, err.Error())
		return
	}

	// 兑换码无效同理：用户输入的问题，不是服务端错误。
	if errors.Is(err, store.ErrCodeInvalid) {
		Fail(w, http.StatusBadRequest, err.Error())
		return
	}

	// 代理地址重复：用户填重了，改一下就行，不是服务故障。
	if errors.Is(err, store.ErrProxyEndpointTaken) {
		Fail(w, http.StatusBadRequest, err.Error())
		return
	}

	var apiErr *gateway.APIError
	if errors.As(err, &apiErr) {
		status := apiErr.Status
		if status < 400 || status > 599 {
			status = http.StatusBadGateway
		}
		Fail(w, status, apiErr.Msg)
		return
	}

	logx.WithContext(r.Context()).Errorf("%s %s 失败: %v", r.Method, r.URL.Path, err)
	Fail(w, http.StatusInternalServerError, "服务内部错误")
}

// Decode 解析 JSON 请求体。
func Decode(r *http.Request, target any) error {
	defer r.Body.Close()

	decoder := json.NewDecoder(http.MaxBytesReader(nil, r.Body, 4<<20))
	if err := decoder.Decode(target); err != nil {
		return fmt.Errorf("请求体不是合法 JSON: %w", err)
	}

	return nil
}

// PathID 取路径参数。
func PathID(r *http.Request, name string) string {
	return r.PathValue(name)
}

// IntQuery 读整型查询参数。
func IntQuery(r *http.Request, name string, fallback int) int {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return fallback
	}
	value, err := strconv.Atoi(raw)
	if err != nil {
		return fallback
	}
	return value
}

// Int64Query 读 64 位整型查询参数。
func Int64Query(r *http.Request, name string) *int64 {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return nil
	}
	value, err := strconv.ParseInt(raw, 10, 64)
	if err != nil {
		return nil
	}
	return &value
}

// Page 是分页响应体。
type Page[T any] struct {
	List     []T `json:"list"`
	Total    int `json:"total"`
	Page     int `json:"page"`
	PageSize int `json:"pageSize"`
}

// write 统一写出响应。
func write(w http.ResponseWriter, status int, payload Envelope) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)

	if err := json.NewEncoder(w).Encode(payload); err != nil {
		logx.Errorf("写出响应失败: %v", err)
	}
}
