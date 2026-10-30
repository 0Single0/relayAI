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
    pub energy_error: Option<String>,
    /// 连登：连续天数。
    pub streak_days: Option<i64>,
    pub streak_error: Option<String>,
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

/// 读取失败保留字段级原因，不能把 401、超时和字段缺失都压成“上游未返回”。
async fn read(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Value, String> {
    let payload = match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        transport::get_json(client, url, headers),
    )
    .await
    {
        Ok(Ok(payload)) => payload,
        Ok(Err(error)) => {
            return Err(format!(
                "查询失败（HTTP {}）：{}",
                error.status,
                super::super::openai_compat::short(&error.message, "上游请求失败")
            ));
        }
        Err(_) => return Err("查询超时，请刷新重试".into()),
    };
    let success = payload.get("code").is_none_or(|code| {
        code.as_i64().is_some_and(|code| code == 0 || code == 200)
            || code.as_str().is_some_and(|code| code == "0" || code == "200")
    });
    if !success {
        return Err(format!(
            "上游返回错误：{}",
            super::super::openai_compat::short(&payload.to_string(), "请求失败")
        ));
    }
    Ok(payload)
}

fn integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn energy(payload: &Value) -> Result<i64, String> {
    let value = payload.get("data").ok_or("上游响应缺少 data，无法读取能量")?;
    integer(value)
        .or_else(|| {
            ["energy", "balance", "total"]
                .iter()
                .find_map(|key| value.get(key).and_then(integer))
        })
        .ok_or_else(|| "上游响应缺少有效的能量数值".into())
}

/// 三个只读请求并行，各自超时或失败不影响其他字段。
pub async fn snapshot(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> GrowthSnapshot {
    let mut headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    headers.extend([
        ("Cache-Control".into(), "no-cache, no-store, max-age=0".into()),
        ("Pragma".into(), "no-cache".into()),
    ]);
    let mut out = GrowthSnapshot::default();
    let energy_url = hosts.chat_url(ENERGY_PATH);
    let streak_url = hosts.chat_url(STREAK_PATH);
    let lottery_url = hosts.chat_url(LOTTERY_SUMMARY_PATH);
    let (energy_result, streak_result, lottery_result) = tokio::join!(
        read(client, &energy_url, &headers),
        read(client, &streak_url, &headers),
        read(client, &lottery_url, &headers),
    );
    match energy_result.and_then(|payload| energy(&payload)) {
        Ok(value) => out.energy = Some(value),
        Err(error) => out.energy_error = Some(error),
    }

    // 连登：天数 + 补签卡 + 档位状态。
    if let Ok(payload) = &streak_result {
        let data = payload.get("data");
        out.streak_days = data
            .and_then(|value| value.get("streak"))
            .and_then(|value| value.get("days"))
            .and_then(integer);
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
    if out.streak_days.is_none() {
        out.streak_error = Some(streak_result
            .err()
            .unwrap_or_else(|| "上游响应缺少有效的连续登录天数".into()));
    }

    // 抽奖次数。
    if let Ok(payload) = lottery_result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_preserves_zero_and_unknown_values() {
        assert_eq!(energy(&json!({"data": {"energy": 0}})), Ok(0));
        assert_eq!(energy(&json!({"data": 42})), Ok(42));
        assert_eq!(energy(&json!({"data": {"energy": "81"}})), Ok(81));
        assert_eq!(energy(&json!({"data": {"balance": 23}})), Ok(23));
        assert!(energy(&json!({"data": {"unrelated": 999}})).is_err());
        assert!(energy(&json!({"data": {"energy": null}})).is_err());
        assert!(energy(&json!({"data": {"energy": "unavailable"}})).is_err());
    }

    #[tokio::test]
    async fn snapshot_reads_distinguish_http_and_application_errors() {
        use axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/unauthorized", get(|| async {
                (axum::http::StatusCode::UNAUTHORIZED, "<html>401 Authorization Required</html>")
            }))
            .route("/business-error", get(|| async {
                axum::Json(json!({"code": 11140, "msg": "account restricted"}))
            }))
            .route("/zero", get(|| async { axum::Json(json!({"code": 0, "data": {"energy": 0}})) }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let headers = Vec::new();
        let http = read(&client, &format!("{base}/unauthorized"), &headers).await.unwrap_err();
        assert!(http.contains("HTTP 401"));
        let business = read(&client, &format!("{base}/business-error"), &headers).await.unwrap_err();
        assert!(business.contains("11140"));
        let zero = read(&client, &format!("{base}/zero"), &headers).await.unwrap();
        assert_eq!(energy(&zero), Ok(0));
        server.abort();
    }
}
