package handler

import (
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"

	"github.com/zeromicro/go-zero/core/logx"
)

// SSE 轮询间隔。
//
// 为什么是轮询 PG 而不是让网关反推：执行状态活在网关内存里，Go 侧只有
// `task_run_items` 这一份真凭实据（网关每出一条结果就 INSERT）。轮询这条表
// 的增量（`id > afterID`）能拿到**已经落库的结果**，进程重启也不丢；
// 反过来让网关反推则要维护第二条通道，还得处理订阅者泄漏。
//
// 400ms 是"看起来实时"与"每秒 2.5 次轻查询"的折中：明细按 1100ms 间隔落库，
// 400ms 的采样不会明显滞后，单条查询走 `task_run_items_run` 索引。
const taskStreamInterval = 400 * time.Millisecond

// SSE 最长连接时长。
//
// 必须有一个上限：浏览器崩溃/断网时服务端不会收到任何信号，goroutine 会挂死。
// 到点后发 `event: done` 并主动关闭，前端 EventSource 会按规范重连，
// 重连后从断点继续（前端把 lastRunItemID 当查询参数带上来）。
const taskStreamMaxDuration = 10 * time.Minute

// 心跳间隔：反向代理常在 30–60s 空闲后断开，注释行可以保活。
const taskStreamHeartbeat = 15 * time.Second

// registerTaskStream 注册任务进度的 SSE 接口（SPEC 7.2 / 9.8）。
//
// 响应**不走统一信封**：SSE 是逐帧文本协议，混入 JSON 信封会破坏 `data:` 语义。
// 错误发生在建立连接之前时仍用信封（前端按普通请求的错误路径处理），
// 建立之后出错则以 `event: failed` 帧下发。
func registerTaskStream(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /tasks/runs/{id}/stream", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "run")
		if !ok {
			return
		}

		// 断点续传：前端重连时把已收到的最后一条明细 id 带上来。
		afterID := int64(httpx.IntQuery(r, "after", 0))

		// 连接建立前先确认这条 run 存在，否则 404 直接给信封，
		// 前端不需要为"id 写错"准备一条 SSE 错误分支。
		if _, err := ctx.Store.TaskRunStatusOf(r.Context(), id); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		flusher, canFlush := w.(http.Flusher)
		if !canFlush {
			httpx.Fail(w, http.StatusInternalServerError, "当前服务器不支持流式响应")
			return
		}

		header := w.Header()
		header.Set("Content-Type", "text/event-stream; charset=utf-8")
		header.Set("Cache-Control", "no-cache")
		header.Set("Connection", "keep-alive")
		// nginx 默认会缓冲上游响应，缓冲后 SSE 就变成"最后一口气全吐出来"。
		header.Set("X-Accel-Buffering", "no")
		w.WriteHeader(http.StatusOK)
		flusher.Flush()

		streamTaskRun(r, w, flusher, ctx, id, afterID)
	})
}

// streamTaskRun 是 SSE 主循环。
func streamTaskRun(
	r *http.Request,
	w http.ResponseWriter,
	flusher http.Flusher,
	ctx *svc.ServiceContext,
	runID int64,
	afterID int64,
) {
	started := time.Now()
	lastBeat := started

	send := func(event string, payload any) bool {
		body, err := json.Marshal(payload)
		if err != nil {
			logx.Errorf("序列化 SSE 帧失败: %v", err)
			return false
		}
		if _, err := fmt.Fprintf(w, "event: %s\ndata: %s\n\n", event, body); err != nil {
			return false
		}
		flusher.Flush()
		return true
	}

	for {
		select {
		case <-r.Context().Done():
			// 浏览器关页/断网：正常退出，不当作错误。
			return
		default:
		}

		if time.Since(started) > taskStreamMaxDuration {
			send("done", map[string]any{"reason": "timeout", "runId": runID})
			return
		}

		items, err := ctx.Store.TaskRunItemsAfter(r.Context(), runID, afterID)
		if err != nil {
			send("failed", map[string]any{"message": "读取任务明细失败"})
			logx.Errorf("SSE 读取任务明细失败: %v", err)
			return
		}

		for _, item := range items {
			afterID = item.ID
			if !send("item", item) {
				return
			}
		}

		run, err := ctx.Store.TaskRunCounters(r.Context(), runID)
		if err != nil {
			send("failed", map[string]any{"message": "读取任务进度失败"})
			logx.Errorf("SSE 读取任务进度失败: %v", err)
			return
		}

		if !send("progress", run) {
			return
		}

		// 终态：把最后一份计数发出去就收尾。前端据此停止，不再依赖轮询。
		if run.Status != "running" {
			send("done", map[string]any{
				"runId":  runID,
				"status": run.Status,
			})
			return
		}

		if time.Since(lastBeat) >= taskStreamHeartbeat {
			lastBeat = time.Now()
			if _, err := fmt.Fprint(w, ": ping\n\n"); err != nil {
				return
			}
			flusher.Flush()
		}

		select {
		case <-r.Context().Done():
			return
		case <-time.After(taskStreamInterval):
		}
	}
}
