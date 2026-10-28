package handler

import (
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"relayai/backend/internal/gateway"
	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"

	"github.com/zeromicro/go-zero/core/logx"
)

// registerAccounts 注册账号相关路由。
func registerAccounts(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /accounts", func(w http.ResponseWriter, r *http.Request) {
		query := store.AccountQuery{
			ChannelID: r.URL.Query().Get("channelId"),
			Keyword:   r.URL.Query().Get("keyword"),
			Page:      httpx.IntQuery(r, "page", 1),
			PageSize:  httpx.IntQuery(r, "pageSize", 50),
		}

		list, total, err := ctx.Store.ListAccounts(r.Context(), query)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 运行态从网关补（SPEC I5：不落库）。
		attachRuntime(r, ctx, list)
		// 用量从 PG 补（今日/累计）—— 账号表要能回答"这个号今天用了多少"。
		attachUsage(r, ctx, list)

		httpx.OK(w, httpx.Page[store.Account]{
			List:     list,
			Total:    total,
			Page:     query.Page,
			PageSize: query.PageSize,
		})
	})

	mux.HandleFunc("GET /accounts/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "账号")
		if !ok {
			return
		}

		account, err := ctx.Store.GetAccount(r.Context(), id)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		list := []store.Account{*account}
		attachRuntime(r, ctx, list)
		httpx.OK(w, list[0])
	})

	mux.HandleFunc("PATCH /accounts/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "账号")
		if !ok {
			return
		}

		var input store.AccountInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		account, err := ctx.Store.UpdateAccount(r.Context(), id, input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)
		httpx.OK(w, account)
	})

	mux.HandleFunc("DELETE /accounts/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "账号")
		if !ok {
			return
		}

		if err := ctx.Store.DeleteAccount(r.Context(), id); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)
		httpx.OKMessage(w, "账号已删除")
	})

	// 刷新凭据：代理到网关（强制刷新，不因"未到期"短路）。
	mux.HandleFunc("POST /accounts/{id}/refresh", func(w http.ResponseWriter, r *http.Request) {
		channelID, uid, ok := accountKey(w, r, ctx)
		if !ok {
			return
		}

		result, err := ctx.Gateway.RefreshCredential(r.Context(), channelID, uid)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, result)
	})

	// 探活：代理到网关（真实打上游）。
	mux.HandleFunc("POST /accounts/{id}/probe", func(w http.ResponseWriter, r *http.Request) {
		channelID, uid, ok := accountKey(w, r, ctx)
		if !ok {
			return
		}

		// 单账号探活：允许用查询参数指定模型（?model=xxx），空则自动挑。
		model := r.URL.Query().Get("model")
		result, err := ctx.Gateway.ProbeAccount(r.Context(), channelID, uid, model)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, result)
	})

	// 全量探活：对**所有启用账号**跑一轮，立即返回统计。
	//
	// 与定时任务走同一个函数（`RunProbeSweepExclusive`）：
	// 两条路径分叉的话，手动探活会慢慢和定时探活行为不一致。
	mux.HandleFunc("POST /accounts/probe-all", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		result, err := RunProbeSweepExclusive(r.Context(), ctx)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, result)
	})

	// 额度快照：代理到网关。
	mux.HandleFunc("GET /accounts/{id}/quota", func(w http.ResponseWriter, r *http.Request) {
		channelID, uid, ok := accountKey(w, r, ctx)
		if !ok {
			return
		}

		quota, err := ctx.Gateway.GetQuota(r.Context(), channelID, uid)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		if quota == nil {
			httpx.OK(w, nil)
			return
		}

		details := make([][2]string, 0, len(quota.Details))
		for _, pair := range quota.Details {
			if len(pair) == 2 {
				details = append(details, [2]string{pair[0], pair[1]})
			}
		}

		httpx.OK(w, map[string]any{
			"label":     quota.Label,
			"unit":      quota.Unit,
			"remaining": quota.Remaining,
			"total":     quota.Total,
			"used":      quota.Used,
			"details":   details,
			"fetchedAt": quota.FetchedAt,
		})
	})

	// 新建账号：填平台 + 凭据（+ 自定义供应商的地址/协议）。
	//
	// 平台在**账号**上，所以这里必须显式给 `platform`；
	// 渠道只负责"这个号算在哪个计价分组里"。
	mux.HandleFunc("POST /accounts", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			ChannelID string `json:"channelId"`
			UID       string `json:"uid"`
			Nickname  string `json:"nickname"`
			APIKey    string `json:"apiKey"`
			// 平台：workbuddy / trae / custom / fake。
			Platform string `json:"platform"`
			// 上游地址与协议在**账号**上：一份计价策略（渠道）下可以有多个
			// 不同上游的 Key。空 baseURL = 用适配器内置默认。
			BaseURL  string `json:"baseUrl"`
			Protocol string `json:"protocol"`
			// 适配器私有参数（workbuddy 的 realm / outbound_identity）。
			Options json.RawMessage `json:"options"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.ChannelID == "" || input.UID == "" {
			httpx.Fail(w, http.StatusBadRequest, "渠道与账号标识都不能为空")
			return
		}
		if input.Platform == "" {
			httpx.Fail(w, http.StatusBadRequest, "必须选择上游平台")
			return
		}
		// 自定义供应商必须有 Key（本地服务可以没有，但这里不做那个例外：
		// 空 Key 打到上游只会得到看不懂的 401）。
		if input.Platform == "custom" && input.APIKey == "" {
			httpx.Fail(w, http.StatusBadRequest, "自定义供应商必须填 API Key")
			return
		}
		if input.BaseURL != "" &&
			!strings.HasPrefix(input.BaseURL, "http://") &&
			!strings.HasPrefix(input.BaseURL, "https://") {
			httpx.Fail(w, http.StatusBadRequest, "Base URL 必须以 http:// 或 https:// 开头")
			return
		}

		// 凭据在这里加密：Go 侧不持有主密钥，交给网关落库。
		if err := ctx.Gateway.CreateAccount(r.Context(), gateway.CreateAccountInput{
			ChannelID: input.ChannelID,
			UID:       input.UID,
			Nickname:  input.Nickname,
			APIKey:    input.APIKey,
			Platform:  input.Platform,
			BaseURL:   input.BaseURL,
			Protocol:  input.Protocol,
			Options:   input.Options,
		}); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		account, err := ctx.Store.FindAccountByUID(r.Context(), input.ChannelID, input.UID)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, account)
	})

	// 登录三件套：按**渠道 + 平台**发起（新增账号时账号还不存在）。
	//
	// 必须带平台：WorkBuddy 与 Trae 的授权流程完全不同，网关要按平台选适配器；
	// 新账号的 `platform` 也由它决定。
	mux.HandleFunc("POST /channels/{id}/login/start", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			Platform string `json:"platform"`
			Realm    string `json:"realm"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}
		if input.Platform == "" {
			httpx.Fail(w, http.StatusBadRequest, "必须指定平台")
			return
		}

		ticket, err := ctx.Gateway.StartLogin(r.Context(), r.PathValue("id"), input.Platform, input.Realm)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, ticket)
	})

	mux.HandleFunc("POST /channels/{id}/login/poll", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			State    string `json:"state"`
			Platform string `json:"platform"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		result, err := ctx.Gateway.PollLogin(r.Context(), r.PathValue("id"), input.State, input.Platform)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		if result.State == "ready" {
			notifyReload(r, ctx)
		}
		httpx.OK(w, result)
	})

	mux.HandleFunc("POST /channels/{id}/login/complete", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			State    string `json:"state"`
			Callback string `json:"callback"`
			Platform string `json:"platform"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.Callback == "" {
			httpx.Fail(w, http.StatusBadRequest, "回调地址不能为空")
			return
		}

		result, err := ctx.Gateway.CompleteLogin(
			r.Context(), r.PathValue("id"), input.State, input.Callback, input.Platform,
		)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		if result.State == "ready" {
			notifyReload(r, ctx)
		}
		httpx.OK(w, result)
	})
}

// attachUsage 从 PG 补账号用量（今日 / 累计 / 最近使用）。
//
// 查不到用量不是错误：新账号本来就没有记录，显示 0 才是对的。
// 所以这里失败只记日志，不打断列表。
func attachUsage(r *http.Request, ctx *svc.ServiceContext, accounts []store.Account) {
	usage, err := ctx.Store.ListAccountUsage(r.Context())
	if err != nil {
		logx.WithContext(r.Context()).Infof("账号用量不可用: %v", err)
		return
	}

	for index := range accounts {
		key := accounts[index].ChannelID + "/" + accounts[index].UID
		if item, ok := usage[key]; ok {
			accounts[index].Usage = &item
		} else {
			// 明确给一个全 0 的占位：前端的"今日 0 次 / 0 tok"就不必自己造默认值。
			accounts[index].Usage = &store.AccountUsage{
				ChannelID: accounts[index].ChannelID,
				UID:       accounts[index].UID,
			}
		}
	}
}

// attachRuntime 从网关状态里补运行态。
//
// 网关不可用时保持 nil，前端显示"状态未知"——**不要编造默认值**，
// 那会让"网关挂了"看起来像"账号都正常"。
//
// 数据来自网关新增的 `accounts[]`（账号级明细）而不是 `channels[]`（渠道级聚合）。
// 用聚合值的话，每个账号都被写成 `Status: "ready"`、`Inflight: 0`，
// 界面上"在飞"永远是 0、并发上限永远缺失 —— 等于列了个假表。
func attachRuntime(r *http.Request, ctx *svc.ServiceContext, accounts []store.Account) {
	status, err := ctx.Gateway.GetStatus(r.Context())
	if err != nil {
		logx.WithContext(r.Context()).Infof("网关状态不可用，账号运行态留空: %v", err)
		return
	}

	// key = channelID + "/" + uid
	byAccount := make(map[string]gateway.AccountRuntime, len(status.Accounts))
	for _, item := range status.Accounts {
		byAccount[item.ChannelID+"/"+item.UID] = item
	}

	for index := range accounts {
		key := accounts[index].ChannelID + "/" + accounts[index].UID
		live, ok := byAccount[key]
		if !ok {
			// 网关不认识这个账号（刚建、还没 reload）：如实标成未知，
			// 不要拿"默认可用"糊过去。
			accounts[index].Runtime = &store.AccountRuntime{
				Status:        "unknown",
				BlockedReasons: []string{"网关尚未加载该账号（可能刚创建，稍后自动刷新）"},
			}
			continue
		}

		state := "ready"
		switch {
		case !live.Enabled:
			state = "disabled"
		case live.RequiresCredential && !live.HasCredential:
			state = "no_credential"
		case len(live.BlockedReasons) > 0:
			state = "cooldown"
		}

		runtime := &store.AccountRuntime{
			Status:           state,
			Inflight:         live.Inflight,
			MaxInflight:      live.MaxInflight,
			BlockedReasons:   live.BlockedReasons,
			StaticCredential: live.StaticCredential,
		}
		if live.CooldownUntilMs > 0 {
			stamp := time.UnixMilli(live.CooldownUntilMs).Format(time.RFC3339)
			runtime.CooldownUntil = &stamp
		}
		// 额度快照：网关内存里的那份，不打上游。
		if live.Quota != nil {
			runtime.Quota = &store.AccountQuota{
				Label:     live.Quota.Label,
				Unit:      live.Quota.Unit,
				Remaining: live.Quota.Remaining,
				Total:     live.Quota.Total,
				Used:      live.Quota.Used,
				FetchedAt: time.UnixMilli(live.Quota.FetchedAtMs).Format(time.RFC3339),
			}
		}
		accounts[index].Runtime = runtime
	}
}

// accountKey 从路径 id 取出账号，返回网关需要的 channel/uid。
func accountKey(w http.ResponseWriter, r *http.Request, ctx *svc.ServiceContext) (string, string, bool) {
	id, ok := parseInt64(w, r.PathValue("id"), "账号")
	if !ok {
		return "", "", false
	}

	account, err := ctx.Store.GetAccount(r.Context(), id)
	if err != nil {
		httpx.FailError(w, r, err)
		return "", "", false
	}

	return account.ChannelID, account.UID, true
}

func logReloadFailure(r *http.Request, err error) {
	logx.WithContext(r.Context()).Errorf("通知网关重载失败（配置已落库，重启后生效）: %v", err)
}
