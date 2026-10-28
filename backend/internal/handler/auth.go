package handler

import (
	"errors"
	"net/http"
	"strings"
	"time"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"

	"github.com/zeromicro/go-zero/core/logx"
)

// 会话令牌的请求头与 Cookie 名。
//
// 用 `Authorization: Bearer <token>`：与大多数工具链的直觉一致，
// 前端 `request.ts` 已经会带 `authorization`，不需要第二套机制。
const sessionHeader = "Authorization"

// bearerToken 从请求头里取出令牌。
//
// 同时接受 `Bearer xxx` 与裸 token：裸 token 是为了让 curl 调试方便，
// 但**不**接受 `Basic`（那是另一种语义，混在一起会让鉴权逻辑变得难懂）。
func bearerToken(r *http.Request) string {
	raw := strings.TrimSpace(r.Header.Get(sessionHeader))
	if raw == "" {
		return ""
	}
	if strings.HasPrefix(strings.ToLower(raw), "bearer ") {
		return strings.TrimSpace(raw[len("bearer "):])
	}
	return raw
}

// registerAuth 注册注册/登录/登出/兑换与用户管理路由。
func registerAuth(mux *http.ServeMux, ctx *svc.ServiceContext) {
	// 注册。第一个注册的人自动成为 admin（开箱即用）。
	mux.HandleFunc("POST /auth/register", func(w http.ResponseWriter, r *http.Request) {
		var input store.RegisterInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		user, err := ctx.Store.RegisterUser(r.Context(), input)
		if err != nil {
			if errors.Is(err, store.ErrEmailTaken) {
				httpx.Fail(w, http.StatusConflict, err.Error())
				return
			}
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		token, err := ctx.Store.CreateSession(r.Context(), user.ID)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{"token": token, "user": user})
	})

	// 登录。
	mux.HandleFunc("POST /auth/login", func(w http.ResponseWriter, r *http.Request) {
		var input struct {
			Email    string `json:"email"`
			Password string `json:"password"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		user, err := ctx.Store.VerifyLogin(r.Context(), input.Email, input.Password)
		if err != nil {
			if errors.Is(err, store.ErrBadCredentials) {
				// 401 而不是 400：语义是"凭据不对"，不是"请求写错了"。
				httpx.Fail(w, http.StatusUnauthorized, err.Error())
				return
			}
			httpx.Fail(w, http.StatusForbidden, err.Error())
			return
		}

		token, err := ctx.Store.CreateSession(r.Context(), user.ID)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{"token": token, "user": user})
	})

	// 当前登录者。
	mux.HandleFunc("GET /auth/me", func(w http.ResponseWriter, r *http.Request) {
		user, ok := requireUser(w, r, ctx)
		if !ok {
			return
		}
		httpx.OK(w, user)
	})

	// 登出。
	mux.HandleFunc("POST /auth/logout", func(w http.ResponseWriter, r *http.Request) {
		token := bearerToken(r)
		if token != "" {
			if err := ctx.Store.DeleteSession(r.Context(), token); err != nil {
				logx.WithContext(r.Context()).Errorf("吊销会话失败: %v", err)
			}
		}
		httpx.OKMessage(w, "已退出登录")
	})

	// 兑换码充值。
	mux.HandleFunc("POST /auth/redeem", func(w http.ResponseWriter, r *http.Request) {
		user, ok := requireUser(w, r, ctx)
		if !ok {
			return
		}

		var input struct {
			Code string `json:"code"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}
		if strings.TrimSpace(input.Code) == "" {
			httpx.Fail(w, http.StatusBadRequest, "兑换码不能为空")
			return
		}

		updated, amount, err := ctx.Store.RedeemForUser(r.Context(), user.ID, input.Code)
		if err != nil {
			if errors.Is(err, store.ErrCodeInvalid) {
				httpx.Fail(w, http.StatusBadRequest, err.Error())
				return
			}
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, map[string]any{
			"user":   updated,
			"amount": amount,
			"msg":    "充值成功",
		})
	})

	// 我的余额流水。
	mux.HandleFunc("GET /auth/transactions", func(w http.ResponseWriter, r *http.Request) {
		user, ok := requireUser(w, r, ctx)
		if !ok {
			return
		}

		page := httpx.IntQuery(r, "page", 1)
		pageSize := httpx.IntQuery(r, "pageSize", 20)
		list, total, err := ctx.Store.ListBalanceTransactions(r.Context(), user.ID, page, pageSize)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, httpx.Page[store.BalanceTransaction]{
			List:     list,
			Total:    total,
			Page:     page,
			PageSize: pageSize,
		})
	})

	// ---- 用户管理（admin） ----

	mux.HandleFunc("GET /users", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		page := httpx.IntQuery(r, "page", 1)
		pageSize := httpx.IntQuery(r, "pageSize", 50)
		list, total, err := ctx.Store.ListUsers(r.Context(), page, pageSize)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, httpx.Page[store.User]{
			List:     list,
			Total:    total,
			Page:     page,
			PageSize: pageSize,
		})
	})

	// 管理员创建用户。
	//
	// 与自助注册分开的接口而不是复用 `/auth/register`：
	// 那个接口有"第一个用户自动成为 admin"的判定，且不接受初始余额 ——
	// 把它改成"如果带了 token 就走管理员路径"会让两条语义纠缠在一个入口里，
	// 权限判断也更容易出错。
	mux.HandleFunc("POST /users", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		var input store.AdminCreateUserInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		user, err := ctx.Store.CreateUserByAdmin(r.Context(), input)
		if err != nil {
			if errors.Is(err, store.ErrEmailTaken) {
				httpx.Fail(w, http.StatusConflict, err.Error())
				return
			}
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		httpx.OK(w, user)
	})

	// 管理员编辑用户（昵称 / 角色 / 启用）。
	mux.HandleFunc("PATCH /users/{id}", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		id, ok := parseInt64(w, r.PathValue("id"), "用户")
		if !ok {
			return
		}

		var input store.AdminUpdateUserInput
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		user, err := ctx.Store.UpdateUserByAdmin(r.Context(), id, input)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, user)
	})

	// 管理员重置密码（同时吊销该用户所有会话）。
	mux.HandleFunc("POST /users/{id}/password", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		id, ok := parseInt64(w, r.PathValue("id"), "用户")
		if !ok {
			return
		}

		var input struct {
			Password string `json:"password"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		if err := ctx.Store.ResetPassword(r.Context(), id, input.Password); err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OKMessage(w, "密码已重置，该用户的所有登录会话已失效")
	})

	// 管理员调整余额。
	mux.HandleFunc("POST /users/{id}/balance", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		id, ok := parseInt64(w, r.PathValue("id"), "用户")
		if !ok {
			return
		}

		var input struct {
			Amount float64 `json:"amount"`
			Note   string  `json:"note"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		user, err := ctx.Store.AdjustBalance(r.Context(), id, input.Amount, input.Note)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, user)
	})

	mux.HandleFunc("GET /redeem-codes", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		page := httpx.IntQuery(r, "page", 1)
		pageSize := httpx.IntQuery(r, "pageSize", 50)
		list, total, err := ctx.Store.ListRedeemCodes(r.Context(), page, pageSize)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		httpx.OK(w, httpx.Page[store.RedeemCode]{
			List:     list,
			Total:    total,
			Page:     page,
			PageSize: pageSize,
		})
	})

	mux.HandleFunc("POST /redeem-codes", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		var input struct {
			Amount  float64 `json:"amount"`
			Count   int     `json:"count"`
			Note    string  `json:"note"`
			ExpiresAt *string `json:"expiresAt"`
		}
		if err := httpx.Decode(r, &input); err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}
		if input.Count <= 0 {
			input.Count = 1
		}

		var expires *time.Time
		if input.ExpiresAt != nil && strings.TrimSpace(*input.ExpiresAt) != "" {
			parsed, err := time.Parse(time.RFC3339, strings.TrimSpace(*input.ExpiresAt))
			if err != nil {
				httpx.Fail(w, http.StatusBadRequest, "过期时间需为 RFC3339 格式")
				return
			}
			expires = &parsed
		}

		list, err := ctx.Store.CreateRedeemCodes(r.Context(), input.Amount, input.Count, input.Note, expires)
		if err != nil {
			httpx.Fail(w, http.StatusBadRequest, err.Error())
			return
		}

		httpx.OK(w, list)
	})
}

// requireUser 取当前登录者；未登录时**已写好 401**，调用方直接 return。
func requireUser(w http.ResponseWriter, r *http.Request, ctx *svc.ServiceContext) (*store.User, bool) {
	token := bearerToken(r)
	if token == "" {
		httpx.Fail(w, http.StatusUnauthorized, "未登录")
		return nil, false
	}

	user, err := ctx.Store.UserByToken(r.Context(), token)
	if err != nil {
		httpx.Fail(w, http.StatusUnauthorized, "登录已过期，请重新登录")
		return nil, false
	}

	return user, true
}

// requireAdmin 取当前登录者并要求 admin 角色。
//
// 403 而不是 401：已登录但没有权限，是两种不同的事。
// 报 401 会让前端把用户踢回登录页，而他其实登录着。
func requireAdmin(w http.ResponseWriter, r *http.Request, ctx *svc.ServiceContext) (*store.User, bool) {
	user, ok := requireUser(w, r, ctx)
	if !ok {
		return nil, false
	}
	if user.Role != "admin" {
		httpx.Fail(w, http.StatusForbidden, "需要管理员权限")
		return nil, false
	}
	return user, true
}
