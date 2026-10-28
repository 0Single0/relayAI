//! 告警通知（SPEC 9.13「通知」/ M8「掉号 1 分钟内收到通知」）。
//!
//! 通知是**尽力而为**的旁路：发不出去只记 stderr，绝不影响网关转发请求。
//! 这一点必须写死在实现里 —— 掉号告警是为了让人去修号，
//! 如果告警本身能把请求打挂，那它比不告警更糟。
//!
//! 去重靠**冷却窗口**而不是"只发一次"：
//! - 掉号是持续状态，每 60 秒巡检都会重新发现同一个号不可用；
//! - 全量只发一次，用户重开面板时反而看不到当前状态；
//! - 每次冷却窗口只发一条，既不会刷屏，又能周期性提醒"还没修好"。

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::gateway::state::SharedGateway;

/// 同一 (kind, subject) 的最小发送间隔。
///
/// 5 分钟是"提醒但不吵"的量级：掉号到修好之间够看好几遍，
/// 又不至于把 webhook 收方刷成日志墙。
const DEDUPE_WINDOW: Duration = Duration::from_secs(300);

/// 单次投递超时。webhook 收方常常写得比它该有的慢，不能拖住网关。
const DELIVER_TIMEOUT: Duration = Duration::from_secs(5);

/// 告警类型（`kind`），前后端与 webhook 收方共用这套字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    /// 账号掉号：凭据缺失或鉴权过期。
    AccountDropped,
    /// 额度不足：余额/积分耗尽被暂停。
    QuotaExhausted,
    /// 错误率突增。
    ErrorBurst,
}

impl AlertKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::AccountDropped => "掉号",
            Self::QuotaExhausted => "额度不足",
            Self::ErrorBurst => "错误率突增",
        }
    }
}

/// 一条待发通知。
#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub kind: AlertKind,
    pub kind_label: &'static str,
    /// 渠道 id（掉号/额度是具体渠道；错误率突增是 `*`）。
    pub channel_id: String,
    /// 账号 uid（错误率突增为空）。
    pub account_uid: String,
    /// 人话说明。
    pub message: String,
    /// 发生时间（epoch 毫秒）。
    pub ts_ms: u64,
}

impl Alert {
    /// 去重键：同类型 + 同一主体。
    fn dedupe_key(&self) -> String {
        format!(
            "{:?}|{}|{}",
            self.kind, self.channel_id, self.account_uid
        )
    }
}

/// 发送器。持有去重表，不持有连接（`reqwest::Client` 复用 `GatewayState` 的那个）。
#[derive(Debug, Default)]
pub struct Notifier {
    sent: Mutex<HashMap<String, Instant>>,
}

impl Notifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// 该不该发：在冷却窗口内返回 false。
    fn should_send(&self, alert: &Alert, now: Instant) -> bool {
        let key = alert.dedupe_key();
        let Ok(mut sent) = self.sent.lock() else {
            // 锁中毒（另一个线程 panic 在临界区）：宁可发也不静默丢弃。
            return true;
        };
        if let Some(last) = sent.get(&key) {
            if now.duration_since(*last) < DEDUPE_WINDOW {
                return false;
            }
        }
        sent.insert(key, now);
        // 顺手清掉过期条目：告警主体数量有限，但仍不该只增不减。
        sent.retain(|_, at| now.duration_since(*at) < DEDUPE_WINDOW * 4);
        true
    }

    /// 投递到 webhook。`url` 为空 = 用户没配 → 什么都不做。
    pub async fn deliver(&self, state: &SharedGateway, url: &str, alert: &Alert) {
        if url.trim().is_empty() {
            return;
        }
        if !self.should_send(alert, Instant::now()) {
            return;
        }

        let payload = serde_json::to_value(alert).unwrap_or(serde_json::Value::Null);
        let request = state
            .client
            .post(url)
            .timeout(DELIVER_TIMEOUT)
            .json(&payload)
            .send()
            .await;

        match request {
            Ok(response) if response.status().is_success() => {
                eprintln!(
                    "[gateway] 已发送{}告警：{}/{}",
                    alert.kind_label, alert.channel_id, alert.account_uid
                );
            }
            Ok(response) => eprintln!(
                "[gateway] {}告警被接收方拒绝（HTTP {}）：{}/{}",
                alert.kind_label,
                response.status().as_u16(),
                alert.channel_id,
                alert.account_uid
            ),
            Err(error) => eprintln!(
                "[gateway] {}告警发送失败（不影响转发）：{error}",
                alert.kind_label
            ),
        }
    }
}

/// 巡检时扫一遍账号，把"掉号"和"额度不足"发出去。
///
/// 用**当前状态**判断而不是记"状态翻转"：进程重启后翻转历史就没了，
/// 而"这个号现在不可用"是重启后依然成立的判断。
/// 重复由 `Notifier` 的冷却窗口吸收。
pub async fn scan_accounts(state: &SharedGateway, url: &str) {
    if url.trim().is_empty() {
        return;
    }

    let now = crate::gateway::timeutil::now_ms();
    for account in state.pool.accounts() {
        if !account.enabled {
            continue;
        }

        // 掉号：要凭据的渠道却拿不到凭据。这是最需要人处理的一类。
        if account.requires_credential && !account.credentials().is_present() {
            state
                .notifier
                .deliver(
                    state,
                    url,
                    &Alert {
                        kind: AlertKind::AccountDropped,
                        kind_label: AlertKind::AccountDropped.label(),
                        channel_id: account.key.channel_id.clone(),
                        account_uid: account.key.uid.clone(),
                        message: "账号没有可用凭据（未登录或凭据解析失败），请重新登录".into(),
                        ts_ms: now,
                    },
                )
                .await;
            continue;
        }

        // 额度不足：被 `QuotaExhausted` 暂停（`balance_until`）才是真的没钱；
        // 普通冷却/熔断是"上游暂时不让用"，不是额度问题，别混为一谈。
        let reasons = account.blocked_reasons("", now);
        if let Some(reason) = reasons.iter().find(|text| text.contains("额度不足")) {
            state
                .notifier
                .deliver(
                    state,
                    url,
                    &Alert {
                        kind: AlertKind::QuotaExhausted,
                        kind_label: AlertKind::QuotaExhausted.label(),
                        channel_id: account.key.channel_id.clone(),
                        account_uid: account.key.uid.clone(),
                        message: reason.clone(),
                        ts_ms: now,
                    },
                )
                .await;
        }
    }
}

/// 错误率突增检查。
///
/// 判据用**进程内计数**（`Stats::requests` / `failures`），不用 PG：
/// 这个告警要的是"现在是不是在连续失败"，查库既慢又多一次依赖。
/// 样本少于 20 次不发 —— 刚启动时 1 次失败就是 100%，那是噪声不是突增。
pub async fn check_error_rate(state: &SharedGateway, url: &str) {
    if url.trim().is_empty() {
        return;
    }

    use std::sync::atomic::Ordering;

    // 两个计数器统计的是**不同阶段**：`requests` 在选号成功后加，
    // `failures` 在选不出号时加（见 `server/openai.rs`）。所以分母是两者之和，
    // 不是 `requests` —— 用 `failures / requests` 会算出 >100% 的比率，
    // "错误率 300%" 这种数字一眼就是坏的，反而没人会去处理。
    let succeeded = state.stats.requests.load(Ordering::Relaxed);
    let failures = state.stats.failures.load(Ordering::Relaxed);
    let total = succeeded + failures;
    if total < 20 {
        return;
    }

    // 30% 是"明显不正常"的线：正常波动远低于它，而真出事时远高于它。
    let rate = failures as f64 / total as f64;
    if rate < 0.3 {
        return;
    }

    state
        .notifier
        .deliver(
            state,
            url,
            &Alert {
                kind: AlertKind::ErrorBurst,
                kind_label: AlertKind::ErrorBurst.label(),
                channel_id: "*".into(),
                account_uid: String::new(),
                message: format!(
                    "进程内累计 {total} 次请求中有 {failures} 次未能发出（{:.0}%），检查上游与账号状态",
                    rate * 100.0
                ),
                ts_ms: crate::gateway::timeutil::now_ms(),
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(kind: AlertKind, uid: &str) -> Alert {
        Alert {
            kind,
            kind_label: kind.label(),
            channel_id: "wb-cn".into(),
            account_uid: uid.into(),
            message: String::new(),
            ts_ms: 0,
        }
    }

    #[test]
    fn same_subject_is_deduped_within_window() {
        let notifier = Notifier::new();
        let now = Instant::now();
        assert!(notifier.should_send(&alert(AlertKind::AccountDropped, "u1"), now));
        assert!(!notifier.should_send(&alert(AlertKind::AccountDropped, "u1"), now));
    }

    #[test]
    fn different_kind_or_subject_is_not_deduped() {
        let notifier = Notifier::new();
        let now = Instant::now();
        assert!(notifier.should_send(&alert(AlertKind::AccountDropped, "u1"), now));
        // 不同账号
        assert!(notifier.should_send(&alert(AlertKind::AccountDropped, "u2"), now));
        // 同账号不同告警类型
        assert!(notifier.should_send(&alert(AlertKind::QuotaExhausted, "u1"), now));
    }

    #[test]
    fn window_expiry_reopens_the_slot() {
        let notifier = Notifier::new();
        let start = Instant::now();
        assert!(notifier.should_send(&alert(AlertKind::AccountDropped, "u1"), start));
        let later = start + DEDUPE_WINDOW + Duration::from_secs(1);
        assert!(
            notifier.should_send(&alert(AlertKind::AccountDropped, "u1"), later),
            "冷却窗口过后必须能再发一次，否则用户修号期间再无提醒"
        );
    }
}
