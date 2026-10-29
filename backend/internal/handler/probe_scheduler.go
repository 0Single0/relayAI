package handler

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/models"
	"relayai/backend/internal/svc"
)

/*
定时探活。

-------------------------------------------------------------------------------
为什么按「分组 × 模型」而不是按账号
-------------------------------------------------------------------------------

上游的限流与权限**按模型**生效。一个分组挂 19 个模型，
只发一句 hello 得不出"这个分组能不能用"：

  · 某个模型挂了 → 探针可能是绿的（恰好挑了另一个）；
  · 某个号对该模型无权限 → 同样看不出来；
  · 反过来，某个模型限流 → 整个分组被标红，其实别的模型好好的。

所以探测粒度落到 (channel, model)，每个模型挑**一个**账号去探。
参考实现（sub2api 的 channel_monitor）也是这个口径：
带 `primary_model` / `extra_models`，结果按模型分别落库。

-------------------------------------------------------------------------------
几个刻意的取舍
-------------------------------------------------------------------------------

 1. **串行 + 间隔**：探活花上游额度，并发打过去像突发流量，更容易被判异常。
 2. **不因单条失败而中断**：一个模型挂了不该让剩下的这轮都跳过。
 3. **结果落 `channel_probes`**：与 `usage_logs` 分开 ——
    那个是给真实请求对账用的，把探活混进去会污染账单。
 4. **每模型只挑一个号**：探活回答"这个模型通不通"，
    号级可用性由网关冷却状态反映，不必靠探活重复确认。
*/

// probeGap 是相邻两次探测之间的间隔。
//
// 1.5 秒：几十个探测能在可接受时间内跑完，又不像突发流量。
const probeGap = 1500 * time.Millisecond

// probeTimeout 是单条探测的超时。
//
// 比网关侧的 45 秒略长：网关自己会先超时，这里只是兜底。
const probeTimeout = 60 * time.Second

// ProbeItem 是一条探测结果。
type ProbeItem struct {
	ChannelID string `json:"channelId"`
	Model     string `json:"model"`
	AccountUID string `json:"accountUid"`
	OK        bool   `json:"ok"`
	LatencyMS int    `json:"latencyMs"`
	Status    string `json:"status"`
	Message   string `json:"message"`
}

// ProbeSweepResult 是一轮探活的统计。
type ProbeSweepResult struct {
	// Total 是本轮探测的 (分组 × 模型) 条数。
	Total int `json:"total"`
	OK    int `json:"ok"`
	Failed int `json:"failed"`
	// Limited 是因上游限流而失败的条数 —— 与"真故障"分开报。
	//
	// 限流不是故障：号还能用，只是那个模型在等。混进 Failed 会让人
	// 以为分组坏了。
	Limited int `json:"limited"`
	Items   []ProbeItem `json:"items"`
}

// classifyProbe 把探测结果归类成可展示的状态。
//
// 分类而不是只存 bool：失败的原因决定"要不要换号 / 换模型 / 补权限"，
// 全归成"失败"等于把可行动的信息丢了。
func classifyProbe(ok bool, detail string) (status, message string) {
	if ok {
		return "ok", ""
	}
	lowered := detail
	switch {
	case containsAny(lowered, "6004", "限流", "频率限制", "rate limit", "429"):
		return "limited", detail
	case containsAny(lowered, "401", "403", "凭据", "unauthorized", "登录"):
		return "auth", detail
	case containsAny(lowered, "额", "quota", "余额", "402"):
		return "quota", detail
	default:
		return "error", detail
	}
}

func containsAny(text string, needles ...string) bool {
	for _, needle := range needles {
		if len(needle) > 0 && containsFold(text, needle) {
			return true
		}
	}
	return false
}

// containsFold 做大小写不敏感的包含判断。
func containsFold(text, needle string) bool {
	if len(needle) > len(text) {
		return false
	}
	lowerText := toLower(text)
	lowerNeedle := toLower(needle)
	for i := 0; i+len(lowerNeedle) <= len(lowerText); i++ {
		if lowerText[i:i+len(lowerNeedle)] == lowerNeedle {
			return true
		}
	}
	return false
}

func toLower(text string) string {
	out := make([]byte, len(text))
	for i := 0; i < len(text); i++ {
		ch := text[i]
		if ch >= 'A' && ch <= 'Z' {
			ch += 'a' - 'A'
		}
		out[i] = ch
	}
	return string(out)
}

// RunProbeOnce 执行一轮探活，返回统计结果。
//
// `trigger` 区分手动与定时，落库后界面能看出"这条是谁触发的"。
func RunProbeOnce(
	ctx context.Context,
	svcCtx *svc.ServiceContext,
	trigger string,
) (*ProbeSweepResult, error) {
	targets, err := svcCtx.Store.ProbeTargets(ctx)
	if err != nil {
		return nil, err
	}

	result := &ProbeSweepResult{Items: make([]ProbeItem, 0, len(targets))}
	for _, target := range targets {
		if ctx.Err() != nil {
			return result, ctx.Err()
		}
		result.Total++

		probeCtx, cancel := context.WithTimeout(ctx, probeTimeout)
		probe, probeErr := svcCtx.Gateway.ProbeAccount(
			probeCtx, target.ChannelID, target.AccountUID, target.Model,
		)
		cancel()

		item := ProbeItem{
			ChannelID:  target.ChannelID,
			Model:      target.OutboundModel,
			AccountUID: target.AccountUID,
		}
		detail := ""
		switch {
		case probeErr != nil:
			detail = probeErr.Error()
		case probe == nil:
			detail = "网关没有返回结果"
		case !probe.OK:
			detail = probe.Error
		}
		item.OK = probe != nil && probeErr == nil && probe.OK
		if probe != nil {
			item.LatencyMS = int(probe.LatencyMs)
		}
		item.Status, item.Message = classifyProbe(item.OK, detail)

		switch item.Status {
		case "ok":
			result.OK++
		case "limited":
			result.Limited++
			result.Failed++
		default:
			result.Failed++
		}
		result.Items = append(result.Items, item)

		// 落库：失败不阻断后续探测，只记日志。
		if err := svcCtx.Store.SaveProbe(ctx, models.ChannelProbes{
			ChannelID:     target.ChannelID,
			Model:         target.Model,
			OutboundModel: target.OutboundModel,
			AccountUID:    target.AccountUID,
			OK:            item.OK,
			LatencyMS:     item.LatencyMS,
			Status:        item.Status,
			Message:       truncate(item.Message, 500),
			Trigger:       trigger,
			CheckedAt:     time.Now(),
		}); err != nil {
			logx.Infof("探活结果落库失败 %s/%s：%v", target.ChannelID, target.Model, err)
		}

		if item.Status != "ok" {
			logx.Infof(
				"探活 %s %s/%s：%s",
				item.Status, target.ChannelID, target.Model,
				truncate(item.Message, 120),
			)
		}

		// 相邻探测之间留空隙（最后一条不用等）。
		select {
		case <-ctx.Done():
			return result, ctx.Err()
		case <-time.After(probeGap):
		}
	}

	return result, nil
}

func min(left, right int) int {
	if left < right {
		return left
	}
	return right
}

func truncate(text string, limit int) string {
	if len(text) <= limit {
		return text
	}
	return text[:limit]
}

// RunProbeScheduler 是定时探活的执行器，阻塞运行。
//
// 间隔与开关**每轮重新读**：用户改了应当下一轮就生效，
// 而不是等进程重启。这也是为什么用「循环 + 重新计算等待」而不是固定 Ticker。
func RunProbeScheduler(ctx context.Context, svcCtx *svc.ServiceContext) {
	// 启动后先等一会儿：让网关先把账号池建起来。
	// 太早探活会得到"账号不存在"，那是假故障。
	if !sleepCtx(ctx, 45*time.Second) {
		return
	}

	for {
		settings, err := svcCtx.Store.GetSettings(ctx)
		if err != nil {
			logx.Infof("定时探活：读设置失败，稍后重试: %v", err)
			if !sleepCtx(ctx, time.Minute) {
				return
			}
			continue
		}

		if !settings.ProbeEnabled {
			if !sleepCtx(ctx, time.Minute) {
				return
			}
			continue
		}

		interval := settings.ProbeIntervalMinutes
		if interval < 5 {
			// 与界面提示的下限一致：间隔太短等于自己烧上游额度。
			interval = 5
		}

		result, err := RunProbeSweepExclusive(ctx, svcCtx, "schedule")
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			if errors.Is(err, ErrProbeBusy) {
				logx.Infof("定时探活跳过：已有一轮在执行")
			} else {
				logx.Infof("定时探活中断: %v", err)
			}
		} else if result.Total > 0 {
			logx.Infof(
				"定时探活完成：%d 条（分组×模型），成功 %d，失败 %d（其中限流 %d）",
				result.Total, result.OK, result.Failed, result.Limited,
			)
		}

		if !sleepCtx(ctx, time.Duration(interval)*time.Minute) {
			return
		}
	}
}

// sleepCtx 等待一段时间，期间可被取消；返回 false 表示被取消。
func sleepCtx(ctx context.Context, d time.Duration) bool {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}

// ErrProbeBusy 表示已有一轮探活在跑。
var ErrProbeBusy = errors.New("已有一轮探活在执行中")

// probeSweepGuard 保证同一时刻只跑一轮。
//
// 手动触发与定时任务可能撞在一起 —— 两轮并发探同一个模型，
// 既浪费额度，也会让上游看到重复请求。
var probeSweepGuard sync.Mutex

// RunProbeSweepExclusive 是带互斥的探活入口。
func RunProbeSweepExclusive(
	ctx context.Context,
	svcCtx *svc.ServiceContext,
	trigger string,
) (*ProbeSweepResult, error) {
	if !probeSweepGuard.TryLock() {
		return nil, ErrProbeBusy
	}
	defer probeSweepGuard.Unlock()

	return RunProbeOnce(ctx, svcCtx, trigger)
}
