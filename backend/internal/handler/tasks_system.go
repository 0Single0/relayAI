package handler

import (
	"net/http"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerTasks 注册任务相关路由（SPEC 7.2）。
//
// 读：渠道能力、任务快照、历史执行记录（Go 直连 PG / 代理网关）。
// 写：发起执行、取消、单独领取（全部代理网关，必须贴着适配器）。
func registerTasks(mux *http.ServeMux, ctx *svc.ServiceContext) {
	// 有运营能力的渠道 + 账号数 + 今日待办（侧边栏二级菜单的数据源）。
	mux.HandleFunc("GET /tasks/channels", func(w http.ResponseWriter, r *http.Request) {
		list, err := ctx.Store.ListTaskChannels(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, list)
	})

	// 实时快照：代理网关（N 次上游请求，慢）。
	mux.HandleFunc("GET /tasks/channels/{id}/live", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		live, err := ctx.Gateway.GetTaskLive(r.Context(), r.PathValue("id"))
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 把网关里"正在执行"的 run_id 补进来（网关内存里有，PG 里也有）。
		for index := range live.Accounts {
			run, err := ctx.Store.RunningTaskRun(r.Context(), r.PathValue("id"), live.Accounts[index].UID)
			if err == nil && run != nil {
				live.Accounts[index].RunID = &run.ID
				live.Accounts[index].Running = true
			}
		}

		httpx.OK(w, live)
	})

	// 发起执行：按账号逐个代理（串行由网关保证）。
	mux.HandleFunc("POST /tasks/runs", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			ChannelID   string   `json:"channelId"`
			AccountUIDs []string `json:"accountUids"`
			Scope       string   `json:"scope"`
			// Codes 非空时只跑这些任务码（界面「执行此任务」用）。
			Codes []string `json:"codes"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.ChannelID == "" || len(input.AccountUIDs) == 0 {
			httpx.Fail(w, http.StatusBadRequest, "渠道与账号都不能为空")
			return
		}
		if input.Scope == "" {
			input.Scope = "all"
		}
		if !validScope(input.Scope) {
			httpx.Fail(w, http.StatusBadRequest, "scope 只能是 all / daily / tasks / checkin")
			return
		}

		batchID, runs, err := ctx.Store.StartTaskRuns(r.Context(), input.ChannelID, input.AccountUIDs, input.Scope)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 通知网关开始执行。账号之间由网关串行（SPEC 11.5）。
		//
		// 必须逐个带上 runId：网关据此把每条明细回写 `task_run_items`、
		// 把最终状态回写 `task_runs`。不带的话执行历史只活在网关内存里，
		// 刷新页面就成了"跑过但查不到"。
		for _, item := range runs {
			runID, _ := item["runId"].(int64)
			accountUID, _ := item["accountUid"].(string)

			started, reason, runErr := ctx.Gateway.StartTaskRun(
				r.Context(), input.ChannelID, accountUID, input.Scope, input.Codes, runID,
			)
			if runErr != nil {
				httpx.FailError(w, r, runErr)
				return
			}
			if !started {
				// 该账号已在执行中：标记这条 run 为失败并说明原因，不阻塞其它账号。
				_ = ctx.Store.FailTaskRun(r.Context(), input.ChannelID, accountUID, reason)
			}
		}

		httpx.OK(w, map[string]any{
			"batchId": batchID,
			"runs":    runs,
		})
	})

	mux.HandleFunc("POST /tasks/runs/{id}/cancel", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "run")
		if !ok {
			return
		}

		// 先读 run 拿到渠道与账号：取消要同时通知网关，而网关按
		// `channel_id + uid` 定位执行中的任务（它不知道 run id）。
		run, _, err := ctx.Store.GetTaskRun(r.Context(), id)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 顺序：先改库再通知网关。
		//
		// 反过来的话，网关可能在两步之间就跑完了并回写 `done`，
		// 用户点了取消却看到"已完成"，会以为按钮没生效。
		// 先改库则库里的 `cancelled` 是最终态，网关的 `status='running'`
		// 条件更新不会把它覆盖回去。
		if err := ctx.Store.CancelTaskRun(r.Context(), id); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		accepted, reason, notifyErr := ctx.Gateway.CancelTaskRun(
			r.Context(), run.ChannelID, run.AccountUID,
		)
		if notifyErr != nil {
			// 库已经标成 cancelled，界面上的反馈是对的；网关没收到通知只会
			// 让本次执行跑完剩下的项。这是可接受的降级，不该让接口报错。
			httpx.OKMessage(w, "已标记取消，但网关未确认（本次执行可能仍会跑完）")
			return
		}
		if !accepted {
			httpx.OKMessage(w, "已标记取消："+reason)
			return
		}

		httpx.OKMessage(w, "已请求取消，当前项结束后停止")
	})

	mux.HandleFunc("GET /tasks/runs", func(w http.ResponseWriter, r *http.Request) {
		query := store.TaskRunQuery{
			ChannelID: r.URL.Query().Get("channel"),
			Account:   r.URL.Query().Get("account"),
			Status:    r.URL.Query().Get("status"),
			Page:      httpx.IntQuery(r, "page", 1),
			PageSize:  httpx.IntQuery(r, "pageSize", 20),
		}

		list, total, err := ctx.Store.ListTaskRuns(r.Context(), query)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, httpx.Page[store.TaskRun]{
			List:     list,
			Total:    total,
			Page:     query.Page,
			PageSize: query.PageSize,
		})
	})

	mux.HandleFunc("GET /tasks/runs/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "run")
		if !ok {
			return
		}

		run, items, err := ctx.Store.GetTaskRun(r.Context(), id)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{"run": run, "items": items})
	})

	// 单独领取：代理网关。
	mux.HandleFunc("POST /tasks/claim", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			ChannelID string `json:"channelId"`
			AccountUID string `json:"accountUid"`
			Code      string `json:"code"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		result, err := ctx.Gateway.ClaimTask(r.Context(), input.ChannelID, input.AccountUID, input.Code)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, result)
	})
}

func validScope(scope string) bool {
	switch scope {
	case "all", "daily", "tasks", "checkin":
		return true
	default:
		return false
	}
}

// registerSystem 注册系统与设置路由。
func registerSystem(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /status", func(w http.ResponseWriter, r *http.Request) {
		status, err := ctx.Gateway.GetStatus(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, status)
	})

	mux.HandleFunc("GET /status/decisions", func(w http.ResponseWriter, r *http.Request) {
		limit := httpx.IntQuery(r, "limit", 50)
		list, err := ctx.Gateway.GetDecisions(r.Context(), limit)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, list)
	})

	mux.HandleFunc("GET /settings", func(w http.ResponseWriter, r *http.Request) {
		settings, err := ctx.Store.GetSettings(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, settings)
	})

	mux.HandleFunc("PATCH /settings", func(w http.ResponseWriter, r *http.Request) {
		var input store.Settings
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.TaskRetentionDays < 7 || input.TaskRetentionDays > 365 {
			httpx.Fail(w, http.StatusBadRequest, "任务记录保留天数需在 7–365 之间")
			return
		}

		if err := ctx.Store.SaveSettings(r.Context(), &input); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		saved, err := ctx.Store.GetSettings(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, saved)
	})
}
