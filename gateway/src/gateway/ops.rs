//! 网关的常驻定时任务（不依赖管理台是否打开）。
//!
//! 为什么放在后端：这些是**系统该自己做的事**，不是"打开面板才做"。
//! 参考项目早期把额度刷新写在前端，结果是"用网关跑了对话，界面上看不到变化"、
//! "切到别的页面刷新就停了"。
//!
//! 定时巡检默认开启（SPEC D10），但**不吃任何 API Key**：它是网关自己发起的
//! 运营动作，不经过 `/v1/*` 入口，也不计入 `usage_logs`。
//! 它消耗的是同一批账号的上游额度，所以与交互式请求共用并发槽、不预留。

use std::time::Duration;

use crate::gateway::{
    state::SharedGateway,
    storage::GatewayPolicy,
    timeutil::now_ms,
};

/// 额度快照超过这个时长就刷新（任务/对话都会改积分，5 分钟粒度够用又不打扰上游）。
const QUOTA_STALE_MS: u64 = 5 * 60 * 1000;
/// 巡检周期。
const TICK: Duration = Duration::from_secs(60);
/// 首次巡检前的等待：把启动时那波请求让过去（目录拉取）。
const STARTUP_DELAY: Duration = Duration::from_secs(20);
/// 账号之间的间隔：避免整点齐发被上游当成异常流量。
const PER_ACCOUNT_GAP: Duration = Duration::from_millis(800);

/// 清理超期记录的最小间隔。
///
/// 保留天数的单位是"天"，没必要每 60 秒删一次；一小时一次足够让库不涨，
/// 又不会把 `DELETE ... WHERE ts < ...` 变成常态负载。
const PURGE_INTERVAL: Duration = Duration::from_secs(3600);

/// 启动常驻定时任务（`main` 时调用一次）。
pub fn spawn(state: SharedGateway) {
    tokio::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;

        // 每 (账号, 动作, 日期) 只自动跑一次；失败项留到整点档再补一次。
        let mut daily: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        // 上次清理的时间戳。0 表示还没清过 → 首轮就清一次。
        let mut last_purge_ms = 0u64;
        // 上次探活的时间戳。0 = 本进程还没跑过。
        let mut last_probe_ms = 0u64;

        loop {
            // 每轮重读策略：用户在设置页关掉巡检或改了间隔，下一轮就生效。
            // 只在启动时读一次的话，界面上改了等于没改（设置项变成摆设）。
            let policy = state.storage.load_policy().await;

            refresh_stale_quotas(&state).await;
            scan_alerts(&state, &policy).await;
            run_daily_actions(&state, &policy, &mut daily).await;
            last_probe_ms =
                probe_accounts(&state, &policy, last_probe_ms).await;

            if now_ms().saturating_sub(last_purge_ms) >= PURGE_INTERVAL.as_millis() as u64 {
                last_purge_ms = now_ms();
                purge_expired(&state, &policy).await;
            }

            tokio::time::sleep(TICK).await;
        }
    });
}

/// 定时探活：对每个启用的账号发一条**最小**请求，返回新的"上次探活时间"。
///
/// 为什么需要它：不主动探活的话，"凭据过期/被封"只有等真实用户请求失败才会发现，
/// 而那时候用户已经吃到 500 了。探活把发现时间提前到故障发生后 N 分钟内。
///
/// 三条克制：
///   1. **只发一句 hello**（`probe_account` 内部就是 `Block::text("hi")`，
///      并优先选免费模型）—— 探活是自费动作，不能拿真实业务请求去试；
///   2. **账号之间留 `PER_ACCOUNT_GAP`**，避免整点齐发被上游当成异常流量；
///   3. **间隔下限 5 分钟**（在 `GatewayPolicy` 侧钳制），别把自己烧穿。
///
/// `policy.probe_enabled = false` 时整段跳过，返回原时间戳（不重置），
/// 这样重新打开后会立刻跑一次，而不是再等一个完整间隔。
async fn probe_accounts(
    state: &SharedGateway,
    policy: &GatewayPolicy,
    last_probe_ms: u64,
) -> u64 {
    if !policy.probe_enabled {
        return last_probe_ms;
    }

    let interval_ms = policy.probe_interval_minutes.max(5) as u64 * 60_000;
    let now = now_ms();

    // `last_probe_ms == 0`（本进程还没探过）时立即跑一轮：
    // 否则重启后要等一个完整间隔，而那期间恰好是最需要知道账号是否可用的时候。
    if last_probe_ms != 0 && now.saturating_sub(last_probe_ms) < interval_ms {
        return last_probe_ms;
    }

    for account in state.pool.accounts() {
        if !account.enabled {
            continue;
        }
        if account.requires_credential && !account.credentials().is_present() {
            continue;
        }
        // 已经在冷却里的号不必再探：探了也还是失败，白烧一次请求。
        if !account.is_ready("", now) {
            continue;
        }

        let _ = crate::gateway::core::dispatch::probe_account(
            state.as_ref(),
            &account,
            None,
            std::time::Duration::from_secs(30),
        )
        .await;

        tokio::time::sleep(PER_ACCOUNT_GAP).await;
    }

    now_ms()
}

/// 跑一遍每日动作：只做**今天还没做**的，且不重复尝试。
///
/// 为什么不只在整点档跑：进程可能刚重启（错过了整点）。
/// 所以启动后先补一次，整点档再给"上次失败"的账号补一次机会。
///
/// `schedule_enabled = false` 时整段跳过。额度刷新不跟着停 ——
/// 那个开关的名字是「定时巡检」，管的是运营动作，不是额度快照。
async fn run_daily_actions(
    state: &SharedGateway,
    policy: &GatewayPolicy,
    done: &mut std::collections::HashMap<String, bool>,
) {
    if !policy.schedule_enabled {
        return;
    }

    let (year, month, day, hour) = crate::gateway::timeutil::local_date_hour();
    let today = format!("{year:04}{month:02}{day:02}");
    let slot = policy.schedule_hours.contains(&hour);

    for account in state.pool.accounts() {
        if !account.enabled {
            continue;
        }
        if account.requires_credential && !account.credentials().is_present() {
            continue;
        }
        if account.cooldown_until_ms() > crate::gateway::timeutil::now_ms() {
            continue;
        }
        let Some(adapter) = state.adapter_for(&account) else {
            continue;
        };
        let snapshot = account.snapshot();
        let Ok(Some(actions)) = adapter.daily_actions(&state.client, &snapshot).await else {
            continue;
        };

        // 先算出这轮要真跑哪几项：一次巡检只建一条 run，
        // 而不是每项建一条（否则历史页会被补跑记录刷满）。
        let mut pending = Vec::new();
        for action in &actions {
            if action.done_today || !action.automatable {
                continue;
            }
            let key = format!("{}|{}|{today}", account.key, action.code);
            match done.get(&key) {
                // 今天试过且成功 → 不再打扰上游。
                Some(true) => continue,
                // 今天试过但失败：只在整点档再给一次机会，避免每 60 秒重试。
                Some(false) if !slot => continue,
                _ => {}
            }
            pending.push(action.code.clone());
        }
        if pending.is_empty() {
            continue;
        }

        // SPEC 11.8：失败必须写 run 记录。参考项目只写 stderr，等于没有 ——
        // 用户看到"该做的都做了"，实际上一项没成，也没地方能查到。
        let run_id = match state
            .storage
            .start_scheduled_run(
                &account.key.channel_id,
                &account.key.uid,
                "daily",
                pending.len() as i32,
            )
            .await
        {
            Ok(id) => Some(id),
            Err(error) => {
                eprintln!("[gateway] 巡检 run 建行失败（本轮不落库）：{}", error.message);
                None
            }
        };

        let mut done_count = 0u32;
        let mut failed_count = 0u32;
        let mut skipped_count = 0u32;
        let mut earned = 0u32;

        for code in pending {
            let key = format!("{}|{}|{today}", account.key, code);
            let outcome = adapter
                .run_daily_action(&state.client, &snapshot, &code)
                .await;

            let (status, message, gained, name) = match outcome {
                Ok(one) => (one.status, one.message, one.earned, one.name),
                Err(error) => (
                    "failed".to_string(),
                    error.describe(),
                    0,
                    code.clone(),
                ),
            };

            match status.as_str() {
                "done" => done_count += 1,
                "skipped" | "not_in_window" => skipped_count += 1,
                _ => failed_count += 1,
            }
            earned += gained;

            eprintln!(
                "[gateway] 每日动作 {name} {}/{} → {status}：{message}",
                account.key.channel_id, account.key.uid
            );

            if let Some(run_id) = run_id {
                if let Err(error) = state
                    .storage
                    .append_task_item(run_id, &code, &name, "daily", &status, &message, gained)
                    .await
                {
                    eprintln!("[gateway] 巡检明细落库失败（run {run_id}）：{}", error.message);
                }
            }

            done.insert(key, status == "done");
            // 签到/领奖都会改积分：顺手刷新额度，面板不用等 5 分钟。
            if status == "done" && code != "daily_chat" {
                if let Ok(Some(quota)) = adapter.fetch_quota(&state.client, &snapshot).await {
                    account.set_quota(Some(quota));
                }
            }
            tokio::time::sleep(Duration::from_millis(policy.daily_gap_ms)).await;
        }

        if let Some(run_id) = run_id {
            if let Err(error) = state
                .storage
                .finish_task_run(
                    run_id,
                    done_count,
                    failed_count,
                    skipped_count,
                    earned,
                    None,
                )
                .await
            {
                eprintln!("[gateway] 巡检 run 收尾失败（run {run_id}）：{}", error.message);
            }
        }
    }
}

/// 刷新"过期"账号的额度快照。串行 + 间隔，冷却中的账号跳过。
async fn refresh_stale_quotas(state: &SharedGateway) {
    let now = now_ms();
    for account in state.pool.accounts() {
        if !account.enabled {
            continue;
        }
        // 没凭据（还没登录）或正在冷却的账号不打扰上游。
        if account.requires_credential && !account.credentials().is_present() {
            continue;
        }
        if account.cooldown_until_ms() > now {
            continue;
        }
        let fresh = account
            .quota()
            .map(|quota| now.saturating_sub(quota.fetched_at_ms) <= QUOTA_STALE_MS)
            .unwrap_or(false);
        if fresh {
            continue;
        }
        let Some(adapter) = state.adapter_for(&account) else {
            continue;
        };
        match adapter
            .fetch_quota(&state.client, &account.snapshot())
            .await
        {
            Ok(snapshot) => {
                account.set_quota(snapshot);
            }
            // 额度刷新失败不该吵用户：下一轮会再试。
            Err(error) => {
                eprintln!(
                    "[gateway] 账号 {} 额度刷新失败（下轮重试）：{}",
                    account.key, error.message
                );
            }
        }
        tokio::time::sleep(PER_ACCOUNT_GAP).await;
    }
}

/// 扫一遍账号与错误率，把告警发出去（SPEC 9.13 / M8）。
///
/// 每轮（60s）都扫：M8 的要求是"掉号 1 分钟内收到通知"，
/// 靠的就是这个周期 + `Notifier` 的冷却窗口去重。
async fn scan_alerts(state: &SharedGateway, policy: &GatewayPolicy) {
    if policy.webhook_url.is_empty() {
        return;
    }
    crate::gateway::notify::scan_accounts(state, &policy.webhook_url).await;
    crate::gateway::notify::check_error_rate(state, &policy.webhook_url).await;
}

/// 按保留天数清理超期记录（SPEC 11.8 / 17）。
///
/// 清理失败只记日志：删不掉旧记录是"库会变大"，不该影响网关转发请求。
async fn purge_expired(state: &SharedGateway, policy: &GatewayPolicy) {
    match state
        .storage
        .purge_task_history(policy.task_retention_days)
        .await
    {
        Ok(0) => {}
        Ok(count) => println!("[gateway] 已清理 {count} 条超期任务记录"),
        Err(error) => eprintln!("[gateway] 清理超期任务记录失败：{}", error.message),
    }

    match state
        .storage
        .purge_usage_logs(policy.log_retention_days)
        .await
    {
        Ok(0) => {}
        Ok(count) => println!("[gateway] 已清理 {count} 条超期用量记录"),
        Err(error) => eprintln!("[gateway] 清理超期用量记录失败：{}", error.message),
    }
}
