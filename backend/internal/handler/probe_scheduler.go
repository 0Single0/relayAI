package handler

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/zeromicro/go-zero/core/logx"

	"relayai/backend/internal/store"
	"relayai/backend/internal/svc"
)

// 定时探活。
//
// ---------------------------------------------------------------------------
// 为什么需要它（一个真实的缺口）
// ---------------------------------------------------------------------------
// 设置页里一直有「定时探活」开关和间隔，但**从来没有任何代码去执行它** ——
// 只有手动点「探活」才真的打一次上游。于是：
//   · 开关是开的，用户以为有人盯着，其实没有；
//   · 凭据过期 / 被封 / 额度耗尽，要等到真实用户请求失败才发现。
//
// 这里补上执行器：按设置的间隔，对每个**启用**的账号发一句 hello。
//
// ---------------------------------------------------------------------------
// 几个刻意的取舍
// ---------------------------------------------------------------------------
//  1. **串行 + 间隔**：探活本身要花上游额度，并发打过去只会让上游看到一阵脉冲，
//     更容易被判成异常流量。串行、每条之间留一点空隙更稳。
//  2. **不因单账号失败而中断**：一个号挂了不该让剩下的号这轮都跳过。
//  3. **结果只记日志，不写库**：`usage_logs` 是给真实请求对账用的，
//     把探活混进去会污染账单与用量统计（探活也真的会消耗 token）。
//     账号可用性由网关的冷却状态反映，列表页读的就是它。
//  4. **跳过停用账号**：停用的号本来就不参与调度，探它纯属浪费额度。

// probeGap 是相邻两次探活之间的间隔。
//
// 1.5 秒：几十个账号的探测能在可接受时间内跑完，又不会在上游看来像突发流量。
const probeGap = 1500 * time.Millisecond

// probeTimeout 是单个账号的探测超时。
//
// 比网关侧的 45 秒略长：网关自己会先超时，这里只是兜底，避免一个卡死的连接拖住整轮。
const probeTimeout = 60 * time.Second

// RunProbeOnce 执行一轮探活，返回统计结果。
//
// 导出是为了让测试与手动触发复用同一份逻辑：定时任务和「立即探活」必须走同一条路径，
// 否则两者行为会慢慢分叉。
func RunProbeOnce(ctx context.Context, svcCtx *svc.ServiceContext) (*ProbeSweepResult, error) {
	accounts, _, err := svcCtx.Store.ListAccounts(ctx, store.AccountQuery{
		Page:     1,
		PageSize: 200,
	})
	if err != nil {
		return nil, err
	}

	// 每个分组配置的探活模型（空 = 让网关按免费优先自动挑）。
	channels, err := svcCtx.Store.ListChannels(ctx)
	if err != nil {
		return nil, err
	}
	probeModelOf := make(map[string]string, len(channels))
	for _, channel := range channels {
		probeModelOf[channel.ID] = channel.ProbeModel
	}

	result := &ProbeSweepResult{}
	for _, account := range accounts {
		// 停用账号不参与调度，探它只是浪费额度。
		if !account.Enabled {
			continue
		}
		result.Total++

		if ctx.Err() != nil {
			return result, ctx.Err()
		}

		probeCtx, cancel := context.WithTimeout(ctx, probeTimeout)
		probe, probeErr := svcCtx.Gateway.ProbeAccount(probeCtx, account.ChannelID, account.UID, probeModelOf[account.ChannelID])
		cancel()

		if probeErr != nil {
			result.Failed++
			logx.Infof(
				"探活失败 %s/%s：%v",
				account.ChannelID, account.UID, probeErr,
			)
		} else if probe == nil || !probe.OK {
			result.Failed++
			detail := ""
			if probe != nil {
				detail = probe.Error
			}
			logx.Infof(
				"探活不通 %s/%s：%s",
				account.ChannelID, account.UID, detail,
			)
		} else {
			result.OK++
		}

		// 相邻探测之间留空隙（最后一个不用等）。
		select {
		case <-ctx.Done():
			return result, ctx.Err()
		case <-time.After(probeGap):
		}
	}

	return result, nil
}

// ProbeSweepResult 是一轮探活的统计。
type ProbeSweepResult struct {
	// Total 是本轮探测的账号数（不含停用）。
	Total int `json:"total"`
	// OK 是探测成功的数量。
	OK int `json:"ok"`
	// Failed 是失败的数量。
	Failed int `json:"failed"`
}

// RunProbeScheduler 是定时探活的执行器，阻塞运行。
//
// 间隔从设置里读**每一轮都要重新读**：用户在设置页改了间隔应当下一轮就生效，
// 而不是等进程重启。这也是为什么用「循环 + 重新计算等待」而不是固定的 `time.Ticker`。
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
			// 关着也要睡一会儿再看：用户可能随时打开，不该等进程重启。
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

		result, err := RunProbeOnce(ctx, svcCtx)
		if err != nil {
			if ctx.Err() != nil {
				return
			}
			logx.Infof("定时探活中断: %v", err)
		} else if result.Total > 0 {
			logx.Infof(
				"定时探活完成：%d 个账号，成功 %d，失败 %d",
				result.Total, result.OK, result.Failed,
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

// probeSweepGuard 保证同一时刻只跑一轮探活。
//
// 手动触发与定时任务可能撞在一起 —— 两轮并发探同一个账号，
// 既浪费额度，也会让上游看到重复请求。
var probeSweepGuard sync.Mutex

// RunProbeSweepExclusive 是带互斥的探活入口。
func RunProbeSweepExclusive(ctx context.Context, svcCtx *svc.ServiceContext) (*ProbeSweepResult, error) {
	if !probeSweepGuard.TryLock() {
		return nil, ErrProbeBusy
	}
	defer probeSweepGuard.Unlock()

	return RunProbeOnce(ctx, svcCtx)
}
