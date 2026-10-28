package handler

import (
	"net/http"
	"strconv"

	"relayai/backend/internal/httpx"
)

// parseInt64 解析路径参数里的整数。
//
// 第二个返回值 false 表示参数不合法；此时**响应已经写好**，调用方直接 return。
// 把"写 400"收进来，是为了避免每个 handler 重复三行样板
// （`id 无效` 的文案也统一由 `label` 决定，不必每处再拼一次）。
func parseInt64(w http.ResponseWriter, raw, label string) (int64, bool) {
	value, err := strconv.ParseInt(raw, 10, 64)
	if err != nil || value <= 0 {
		httpx.Fail(w, http.StatusBadRequest, label+" id 无效")
		return 0, false
	}
	return value, true
}
