package handler

import (
	"net/http"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// registerProxies 注册代理相关路由。
//
// 全部要管理员：代理配置能改所有账号的出口 IP，属于基础设施级权限。
func registerProxies(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /proxies", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		list, err := ctx.Store.ListProxies(r.Context())
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, list)
	})

	mux.HandleFunc("POST /proxies", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		input, ok := decodeProxyInput(w, r, ctx)
		if !ok {
			return
		}

		proxy, err := ctx.Store.CreateProxy(r.Context(), input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, proxy)
	})

	mux.HandleFunc("PATCH /proxies/{id}", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		id, ok := parseInt64(w, r.PathValue("id"), "代理")
		if !ok {
			return
		}

		input, ok := decodeProxyInput(w, r, ctx)
		if !ok {
			return
		}

		proxy, err := ctx.Store.UpdateProxy(r.Context(), id, input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, proxy)
	})

	mux.HandleFunc("DELETE /proxies/{id}", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		id, ok := parseInt64(w, r.PathValue("id"), "代理")
		if !ok {
			return
		}

		if err := ctx.Store.DeleteProxy(r.Context(), id); err != nil {
			httpx.FailError(w, r, err)
			return
		}
		notifyReload(r, ctx)
		httpx.OKMessage(w, "代理已删除；原先绑定的账号改为直连")
	})

	// 账号绑定/解绑代理。
	mux.HandleFunc("POST /accounts/{id}/proxy", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		accountID, ok := parseInt64(w, r.PathValue("id"), "账号")
		if !ok {
			return
		}

		var input struct {
			// ProxyID 为 null 表示解绑（走直连）。
			ProxyID *int64 `json:"proxyId"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if err := ctx.Store.SetAccountProxy(r.Context(), accountID, input.ProxyID); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		// 代理变了必须让网关重载账号：出站客户端是按账号选的，
		// 不重载的话旧配置还会继续用，表现为"改了没用"。
		notifyReload(r, ctx)
		httpx.OKMessage(w, "已更新账号代理")
	})
}

// decodeProxyInput 解析并处理密码加密。
//
// 密码在这里就送去网关加密，**明文字符串不落库、也不进返回值**。
// 编辑时留空 = 不改密码（`PasswordEnc` 保持 nil）。
func decodeProxyInput(w http.ResponseWriter, r *http.Request, ctx *svc.ServiceContext) (store.ProxyInput, bool) {
	var input store.ProxyInput
	if err := httpx.Decode(r, &input); err != nil {
		httpx.Fail(w, http.StatusBadRequest, err.Error())
		return store.ProxyInput{}, false
	}

	if input.Password != nil && *input.Password != "" {
		cipher, err := ctx.Gateway.SealSecret(r.Context(), *input.Password)
		if err != nil {
			httpx.FailError(w, r, err)
			return store.ProxyInput{}, false
		}
		input.PasswordEnc = &cipher
	}

	return input, true
}
