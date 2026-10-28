//! 成长体系里除签到 / 猫猫旅行之外的三个动作。
//!
//! 参考实现（workbuddy2api-hub 的 `wb_tasks.py`）把它们分得很清楚，
//! 我们也按同样的边界拆开，避免 `daily.rs` 无限膨胀：
//!
//! | 动作 | 上游接口 | 说明 |
//! | --- | --- | --- |
//! | 加油站 | `GET /v2/activity/growth/energy` | 只读快照：当前能量余额 |
//! | 连登管家 | `GET /activity/growth/streak` | 连登天数 / 补签卡 / 档位状态 |
//! | 抽奖 | `GET /activity/growth/lottery/summary` | 剩余抽奖次数 |
//!
//! **只做"读 + 可安全执行"的部分**：
//!   · 补签卡、档位兑换、抽奖都要 POST，且上游对次数与顺序有约束；
//!   · 这些动作一旦点错会消耗掉补签卡/抽奖次数，无法撤回。
//!
//! 所以这里的策略是：**能读的都读出来展示**，写操作只做幂等安全的那一种
//! （抽奖本身就是消耗行为，但"有次数就抽掉"是收益确定的，参考实现也是这么做的）。

use serde_json::{json, Value};

use crate::gateway::channels::AccountSnapshot;
use crate::gateway::transport;

use super::identity::{self, Hosts, Outbound, Realm};

/// 加油站（能量余额）。
const ENERGY_PATH: &str = "/v2/activity/growth/energy";
/// 连登管家（天数 / 补签卡 / 档位）。
const STREAK_PATH: &str = "/activity/growth/streak";
/// 抽奖概览与抽一次。
const LOTTERY_SUMMARY_PATH: &str = "/activity/growth/lottery/summary";
const LOTTERY_DRAW_PATH: &str = "/activity/growth/lottery/draw";

/// 成长体系的只读快照。
///
/// 全字段用 `Option`：上游任何一个接口失败都不该让整块显示成"0"——
/// 那会让人以为是真实值（比如"连登 0 天"而不是"查不到"）。
#[derive(Debug, Default, Clone)]
pub struct GrowthSnapshot {
    /// 加油站：当前能量余额。
    pub energy: Option<i64>,
    /// 连登：连续天数。
    pub streak_days: Option<i64>,
    /// 连登：补签卡余额。
    pub makeup_cards: Option<i64>,
    /// 连登：7 / 14 / 28 天档位状态（`locked` / `claimable` / `claimed`）。
    pub tier_7d: Option<String>,
    pub tier_14d: Option<String>,
    pub tier_28d: Option<String>,
    /// 抽奖：剩余次数。
    pub lottery_chances: Option<i64>,
}

impl GrowthSnapshot {
    /// 是否一条都没读到（用于界面判断"整块不可用"）。
    pub fn is_empty(&self) -> bool {
        self.energy.is_none()
            && self.streak_days.is_none()
            && self.makeup_cards.is_none()
            && self.lottery_chances.is_none()
    }
}

/// 读一次成长快照。任何一个接口失败都只是该字段为 `None`。
pub async fn snapshot(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> GrowthSnapshot {
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    let mut out = GrowthSnapshot::default();

    // 加油站能量：字段名在上游是 `energy`（数字）或对象，两种都兼容。
    if let Ok(payload) = transport::get_json(client, &hosts.chat_url(ENERGY_PATH), &headers).await {
        out.energy = payload
            .get("data")
            .and_then(|data| {
                data.get("energy")
                    .or_else(|| data.get("balance"))
                    .or_else(|| data.get("total"))
            })
            .and_then(Value::as_i64);
    }

    // 连登：天数 + 补签卡 + 档位状态。
    if let Ok(payload) = transport::get_json(client, &hosts.chat_url(STREAK_PATH), &headers).await {
        let data = payload.get("data");
        out.streak_days = data
            .and_then(|value| value.get("streak"))
            .and_then(|value| value.get("days"))
            .and_then(Value::as_i64);
        out.makeup_cards = data
            .and_then(|value| value.get("makeup_cards"))
            .and_then(|value| value.get("balance"))
            .and_then(Value::as_i64);
        let redemption = data.and_then(|value| value.get("redemption_status"));
        let tier = |key: &str| {
            redemption
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        out.tier_7d = tier("tier_7d_status");
        out.tier_14d = tier("tier_14d_status");
        out.tier_28d = tier("tier_28d_status");
    }

    // 抽奖次数。
    if let Ok(payload) =
        transport::get_json(client, &hosts.chat_url(LOTTERY_SUMMARY_PATH), &headers).await
    {
        out.lottery_chances = payload
            .get("data")
            .and_then(|value| value.get("chances"))
            .and_then(Value::as_i64);
    }

    out
}

/// 把剩余抽奖次数一次性抽完，返回（抽了几次, 说明）。
///
/// 为什么"全抽"而不是"抽一次"：
///   · 抽奖次数每天刷新，留着不用就作废；
///   · 参考实现也是循环抽到 0 次为止（`# 3. 自动抽奖闭环 (有 chances 则全抽完)`）；
///   · 每次都是确定收益，不存在"抽了更亏"的情况。
///
/// 上限 20 次：防止上游返回一个异常大的 chances 时把额度无限刷下去。
pub async fn draw_all(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> (u32, String) {
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );

    let chances = match transport::get_json(client, &hosts.chat_url(LOTTERY_SUMMARY_PATH), &headers)
        .await
    {
        Ok(payload) => payload
            .get("data")
            .and_then(|value| value.get("chances"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        Err(error) => {
            return (
                0,
                format!("查询抽奖次数失败：{}", error.message),
            )
        }
    };

    if chances <= 0 {
        return (0, "今天没有可用的抽奖次数".to_string());
    }

    let mut drawn = 0u32;
    for _ in 0..chances.min(20) {
        match transport::post_json(
            client,
            &hosts.chat_url(LOTTERY_DRAW_PATH),
            &headers,
            &json!({}),
        )
        .await
        {
            Ok(_) => drawn += 1,
            Err(error) => {
                // 抽到一半失败就停：继续重试可能把剩余次数也耗在失败上。
                return (
                    drawn,
                    format!("已抽 {drawn} 次后中断：{}", error.message),
                );
            }
        }
    }

    (drawn, format!("已抽奖 {drawn} 次"))
}
