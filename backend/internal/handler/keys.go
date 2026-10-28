package handler

import (
	"net/http"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerKeys 注册密钥相关路由。
//
// 明文只在创建与轮换的响应里出现一次（SPEC I2）。
func registerKeys(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /keys", func(w http.ResponseWriter, r *http.Request) {
		list, err := ctx.Store.ListKeys(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 用量按 Key 聚合后附在列表上。
		//
		// 查询失败不阻断列表：看不到用量比看不到密钥列表好得多。
		usage, err := ctx.Store.ListKeyUsage(r.Context())
		if err != nil {
			logx.WithContext(r.Context()).Infof("密钥用量不可用: %v", err)
			usage = nil
		}

		type keyWithUsage struct {
			store.APIKey
			Usage *store.KeyUsage `json:"usage,omitempty"`
		}

		out := make([]keyWithUsage, 0, len(list))
		for _, key := range list {
			item := keyWithUsage{APIKey: key}
			if value, ok := usage[key.ID]; ok {
				item.Usage = &value
			}
			out = append(out, item)
		}

		httpx.OK(w, out)
	})

	mux.HandleFunc("POST /keys", func(w http.ResponseWriter, r *http.Request) {
		// 拿当前用户：Key **必须**有归属，否则用它产生的用量扣不到任何人头上。
		//
		// 这是一个真实的缺口：`CreateKey` 从来没写过 `user_id`，
		// 于是所有 Key 的归属都是 NULL，结算时那条用量只能被跳过 ——
		// 表现就是"消费了但余额没少"。
		user, ok := requireAdmin(w, r, ctx)
		if !ok {
			return
		}

		var input store.KeyInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if input.Name == "" {
			httpx.Fail(w, http.StatusBadRequest, "密钥名称不能为空")
			return
		}

		// **必须绑一个渠道。** 密钥的意义就是"让某个客户端走某个通道"，
		// 允许留空等于允许它走任意通道，那"便宜的 Key / 稳定的 Key"分流就不成立。
		// 前端已限制，这里再兜一次：接口可以绕过界面被直接调用。
		if len(input.ChannelAllow) != 1 || input.ChannelAllow[0] == "" {
			httpx.Fail(w, http.StatusBadRequest, "必须选择一个渠道")
			return
		}

		input.UserID = user.ID
		key, plaintext, err := ctx.Store.CreateKey(r.Context(), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{
			"key":       key,
			"plaintext": plaintext,
		})
	})

	mux.HandleFunc("PATCH /keys/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "密钥")
		if !ok {
			return
		}

		var input store.KeyInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		key, err := ctx.Store.UpdateKey(r.Context(), id, input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, key)
	})

	mux.HandleFunc("DELETE /keys/{id}", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "密钥")
		if !ok {
			return
		}

		if err := ctx.Store.DeleteKey(r.Context(), id); err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OKMessage(w, "密钥已删除")
	})

	mux.HandleFunc("POST /keys/{id}/rotate", func(w http.ResponseWriter, r *http.Request) {
		id, ok := parseInt64(w, r.PathValue("id"), "密钥")
		if !ok {
			return
		}

		plaintext, err := ctx.Store.RotateKey(r.Context(), id)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, map[string]any{"plaintext": plaintext})
	})
}
