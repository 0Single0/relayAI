package handler

import (
	"net/http"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/svc"
)

func registerAccountModels(mux *http.ServeMux, ctx *svc.ServiceContext) {
	mux.HandleFunc("GET /accounts/{id}/models", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}
		channelID, uid, ok := accountKey(w, r, ctx)
		if !ok {
			return
		}
		models, err := ctx.Gateway.GetAccountModels(r.Context(), channelID, uid)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}
		httpx.OK(w, models)
	})
}
