package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/httpx"
	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// openRouterModelsURL 是公开的模型价格清单（无需鉴权）。
const openRouterModelsURL = "https://openrouter.ai/api/v1/models"

// registerPricing 注册定价相关路由。
func registerPricing(mux *http.ServeMux, ctx *svc.ServiceContext) {
	// 用内置价格表填充"从未定价"的模型。
	mux.HandleFunc("POST /models/prices/builtin", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		rate := currentUSDToCNY(r, ctx)
		filled, err := ctx.Store.ApplyBuiltinPrices(r.Context(), rate)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)
		httpx.OK(w, map[string]any{
			"filled":    filled,
			"usdToCny":  rate,
			"message":   fmt.Sprintf("已为 %d 个未定价模型填入内置价格", filled),
		})
	})

	// 从 OpenRouter 拉最新价格并覆盖。
	mux.HandleFunc("POST /models/prices/openrouter", func(w http.ResponseWriter, r *http.Request) {
		if _, ok := requireAdmin(w, r, ctx); !ok {
			return
		}

		models, err := fetchOpenRouterModels(r.Context())
		if err != nil {
			httpx.Fail(w, http.StatusBadGateway, fmt.Sprintf("拉取 OpenRouter 价格失败：%v", err))
			return
		}

		rate := currentUSDToCNY(r, ctx)
		updated, err := ctx.Store.ApplyOpenRouterPrices(r.Context(), models, rate)
		if err != nil {
			httpx.FailError(w, r, err)
			return
		}

		notifyReload(r, ctx)
		httpx.OK(w, map[string]any{
			"updated":  updated,
			"fetched":  len(models),
			"usdToCny": rate,
			"message": fmt.Sprintf(
				"从 OpenRouter 拉到 %d 个模型，更新了 %d 个本地模型的价格",
				len(models), updated,
			),
		})
	})
}

// currentUSDToCNY 读汇率设置（缺省 7.2）。
func currentUSDToCNY(r *http.Request, ctx *svc.ServiceContext) float64 {
	settings, err := ctx.Store.GetSettings(r.Context())
	if err != nil || settings.USDToCNY <= 0 {
		return 7.2
	}
	return settings.USDToCNY
}

// fetchOpenRouterModels 拉 OpenRouter 的模型清单。
//
// 超时 20 秒：那个接口返回的是几 MB 的 JSON，慢网络下 10 秒可能不够。
func fetchOpenRouterModels(ctx context.Context) ([]store.OpenRouterModel, error) {
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()

	request, err := http.NewRequestWithContext(ctx, http.MethodGet, openRouterModelsURL, nil)
	if err != nil {
		return nil, err
	}

	response, err := http.DefaultClient.Do(request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()

	if response.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP %d", response.StatusCode)
	}

	// 限流读 16MB：正常响应约 2–3MB，读到 16MB 还没完说明返回了异常内容。
	body, err := io.ReadAll(io.LimitReader(response.Body, 16<<20))
	if err != nil {
		return nil, err
	}

	var payload struct {
		Data []store.OpenRouterModel `json:"data"`
	}
	if err := json.Unmarshal(body, &payload); err != nil {
		return nil, fmt.Errorf("解析响应失败：%w", err)
	}

	logx.Infof("OpenRouter 返回 %d 个模型", len(payload.Data))
	return payload.Data, nil
}
