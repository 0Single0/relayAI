//! 每日动作：**国内版与国际版各自的动作不重叠**，由这里按 realm 分开。
//!
//! 数据与规则来自参考实现（MIT，`wb_scheduler.py` / `wb_tasks.py`）。
//!
//! | realm | 动作 | 上游 |
//! |---|---|---|
//! | cn | 每日签到（领积分，幂等） | `POST /v2/billing/meter/daily-checkin` |
//! | cn | 猫猫旅行（idle→depart / arrived→claim） | `/activity/growth/buddy/travel/{status,config,depart,claim}` |
//! | intl | 每日活跃对话（送 30/50 积分） | 对话接口发一条最简请求 |
//!
//! 参考实现的定时口径是 09:00 / 21:00 各跑一次（忘了签就真的少积分），
//! 所以真正的"自动"交给 `gateway::ops` 的常驻定时器，这里只提供**查询 + 执行**。

use serde_json::{json, Value};

use crate::gateway::channels::{AccountSnapshot, DailyAction, TaskRunResult};
use crate::gateway::transport;

use super::identity::{self, Hosts, Outbound, Realm};

/// 签到（billing 域，与对话不是同一套头）。
const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";
/// 签到活动状态：`today_checked_in` / `streak_days` / `daily_credit` / `today_credit`。
const CHECKIN_STATUS_PATH: &str = "/v2/billing/meter/checkin-activity-status";

/// 猫猫旅行（activity 域）。
const TRAVEL_STATUS_PATH: &str = "/activity/growth/buddy/travel/status";
const TRAVEL_CONFIG_PATH: &str = "/activity/growth/buddy/travel/config";
const TRAVEL_DEPART_PATH: &str = "/activity/growth/buddy/travel/depart";
const TRAVEL_CLAIM_PATH: &str = "/activity/growth/buddy/travel/claim";

/// 每日活跃对话用最简请求：模型固定成一个确定存在的，只要"发出去并跑通"。
const DAILY_CHAT_MODEL: &str = "deepseek-v4.1-flash";
/// 上游对"今天已签到"的幂等码。
const ALREADY_DONE_CODE: i64 = 10001;

pub const SIGN_IN: &str = "sign_in";
/// 成长抽奖：把当天剩余次数抽完（收益确定，参考实现也是全抽）。
pub const LOTTERY: &str = "lottery";
pub const CAT_TRAVEL: &str = "cat_travel";
pub const DAILY_CHAT: &str = "daily_chat";

/// 该账号今天有哪些每日动作。**不跨 realm 混列**。
pub async fn actions(
    client: &reqwest::Client,
    hosts: &Hosts,
    realm: Realm,
    account: &AccountSnapshot,
) -> Vec<DailyAction> {
    match realm {
        Realm::Cn => {
            let mut list = Vec::new();
            list.push(checkin_action(client, hosts, account).await);
            list.push(travel_action(client, hosts, account).await);
            // 抽奖：**有剩余次数才列出来**。
            // 没次数时列一条"今天抽完了"没有信息量，还会让"每日动作 N 项"虚高。
            if let Some(action) = lottery_action(client, hosts, account).await {
                list.push(action);
            }
            list
        }
        Realm::Intl => vec![chat_action(account)],
    }
}

/// 执行一个动作。返回结果行（失败原因在 message 里，不抛给上层弹窗）。
pub async fn run(
    client: &reqwest::Client,
    hosts: &Hosts,
    realm: Realm,
    account: &AccountSnapshot,
    code: &str,
) -> TaskRunResult {
    match (realm, code) {
        (Realm::Cn, SIGN_IN) => checkin(client, hosts, account).await,
        (Realm::Cn, CAT_TRAVEL) => travel(client, hosts, account).await,
        (Realm::Intl, DAILY_CHAT) => daily_chat(client, hosts, account).await,
        // realm 与动作对不上：如实说，别假装做过（CN 没有每日对话、Intl 没有签到）。
        (realm, code) => result(
            code,
            daily_name(code),
            "failed",
            format!(
                "{:?} 版没有「{}」这个动作（动作按版本分开）",
                realm,
                daily_name(code)
            ),
            0,
        ),
    }
}

// ── 国内版：签到 ──────────────────────────────────────────────────────────

async fn checkin_action(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> DailyAction {
    let headers = identity::billing_headers(hosts, account, None);
    let payload = transport::post_json(
        client,
        &hosts.billing_url(CHECKIN_STATUS_PATH),
        &headers,
        &json!({}),
    )
    .await;
    // 查不到状态时按"今天没签"处理：签到接口是幂等的（10001 = 已签），
    // 多签一次没有副作用，比因为查询失败而漏签好。
    let data = payload
        .ok()
        .and_then(|value| value.get("data").cloned())
        .unwrap_or(Value::Null);
    let done = data
        .get("today_checked_in")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let streak = data.get("streak_days").and_then(Value::as_u64).unwrap_or(0);
    let daily = data.get("daily_credit").and_then(Value::as_u64).unwrap_or(0);
    let today = data.get("today_credit").and_then(Value::as_u64).unwrap_or(0);

    let mut note = Vec::new();
    if done {
        note.push("今天已签".to_string());
    } else {
        note.push("今天还没签".to_string());
    }
    if streak > 0 {
        note.push(format!("连续 {streak} 天"));
    }
    if today > 0 {
        note.push(format!("今天已得 {today} 积分"));
    } else if daily > 0 {
        note.push(format!("每日 {daily} 积分"));
    }
    DailyAction {
        code: SIGN_IN.into(),
        name: "每日签到".into(),
        action: "签到".into(),
        done_today: done,
        note: note.join(" · "),
        automatable: true,
    }
}

async fn checkin(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> TaskRunResult {
    let headers = identity::billing_headers(hosts, account, None);
    match transport::post_json(client, &hosts.billing_url(CHECKIN_PATH), &headers, &json!({})).await
    {
        Ok(value) => {
            let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
            let credit = value
                .get("data")
                .and_then(|data| data.get("credit"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            if code == 0 {
                result(SIGN_IN, "每日签到", "done", "签到成功".into(), credit)
            } else if code == ALREADY_DONE_CODE {
                result(SIGN_IN, "每日签到", "done", "今天已经签过了".into(), 0)
            } else {
                let msg = value.get("msg").and_then(Value::as_str).unwrap_or("上游未说明原因");
                result(SIGN_IN, "每日签到", "failed", format!("上游拒绝：{msg}"), 0)
            }
        }
        Err(error) => result(SIGN_IN, "每日签到", "failed", error.describe(), 0),
    }
}

// ── 国内版：猫猫旅行 ──────────────────────────────────────────────────────

async fn travel_action(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> DailyAction {
    let state = travel_state(client, hosts, account).await;
    let (state, done, note, action) = match state {
        // 只有 "idle" 且今日次数未用完才需要派出；arrived 是等你去领奖。
        Some(value) => {
            let state = value
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let limit_reached = value
                .get("daily_limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match state.as_str() {
                "arrived" => (
                    state,
                    false,
                    "旅行归来，可以领奖".to_string(),
                    "领取奖励".to_string(),
                ),
                "idle" if limit_reached => (
                    state,
                    true,
                    "今日已完成旅行，明日 00:00 刷新".to_string(),
                    "派出旅行".to_string(),
                ),
                "idle" => (state, false, "可以派猫猫出去旅行".to_string(), "派出旅行".to_string()),
                "traveling" => (
                    state,
                    false,
                    "猫猫正在旅行途中，稍后再来".to_string(),
                    "查看".to_string(),
                ),
                other => {
                    // 先把说明算出来再用 state：`other` 借的就是它（借用不能在 move 之后）。
                    let note = format!("当前状态：{other}");
                    (state, false, note, "查看".to_string())
                }
            }
        }
        None => (
            "unknown".to_string(),
            false,
            "查不到旅行状态".to_string(),
            "查看".to_string(),
        ),
    };
    DailyAction {
        code: CAT_TRAVEL.into(),
        name: "猫猫旅行".into(),
        action,
        // traveling 不算"今天做完了"（回来还要领奖），只有次数用完才是。
        done_today: done && state != "arrived",
        note,
        automatable: true,
    }
}

/// 成长抽奖：剩余次数 > 0 才返回一项，否则 `None`。
///
/// 为什么用 `Option` 而不是永远返回一项：
/// 抽奖不在"每天必做"的清单里 —— 没次数时它只是一条噪音，
/// 还会让顶部"每日动作 N 项"的数字虚高。
async fn lottery_action(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Option<DailyAction> {
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    let payload = transport::get_json(
        client,
        &hosts.chat_url("/activity/growth/lottery/summary"),
        &headers,
    )
    .await
    .ok()?;
    let chances = payload
        .get("data")
        .and_then(|value| value.get("chances"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if chances <= 0 {
        return None;
    }

    Some(DailyAction {
        code: LOTTERY.into(),
        name: "成长抽奖".into(),
        action: "全部抽掉".into(),
        done_today: false,
        note: format!("剩余 {chances} 次，抽奖次数每天刷新"),
        automatable: true,
    })
}
async fn travel_state(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Option<Value> {
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    transport::get_json(client, &hosts.chat_url(TRAVEL_STATUS_PATH), &headers)
        .await
        .ok()
        .and_then(|payload| payload.get("data").cloned())
}

async fn travel(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> TaskRunResult {
    let headers = || {
        identity::chat_headers(
            &account.channel_id,
            Realm::Cn,
            Outbound::Desktop,
            hosts,
            account,
            1,
        )
    };
    let Some(state) = travel_state(client, hosts, account).await else {
        return result(
            CAT_TRAVEL,
            "猫猫旅行",
            "failed",
            "查不到旅行状态（上游没给 data）".into(),
            0,
        );
    };
    let current = state.get("state").and_then(Value::as_str).unwrap_or("");
    match current {
        // 已归来 → 领奖。
        "arrived" => {
            match transport::post_json(
                client,
                &hosts.chat_url(TRAVEL_CLAIM_PATH),
                &headers(),
                &json!({}),
            )
            .await
            {
                Ok(value) => {
                    let earned = value
                        .get("data")
                        .and_then(|data| data.get("reward_credit"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;
                    result(CAT_TRAVEL, "猫猫旅行", "done", "旅行归来领奖成功".into(), earned)
                }
                Err(error) => result(CAT_TRAVEL, "猫猫旅行", "failed", error.describe(), 0),
            }
        }
        // 闲着 → 派出。**必须带 location_id**：空 body 会被上游 400 invalid request。
        "idle" => {
            if state
                .get("daily_limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return result(
                    CAT_TRAVEL,
                    "猫猫旅行",
                    "done",
                    "今日已完成旅行，明日 00:00 刷新".into(),
                    0,
                );
            }
            let location = travel_location(client, hosts, account).await;
            match transport::post_json(
                client,
                &hosts.chat_url(TRAVEL_DEPART_PATH),
                &headers(),
                &json!({ "location_id": location.id }),
            )
            .await
            {
                Ok(value) if value.get("code").and_then(Value::as_i64) == Some(0) => {
                    let name = value
                        .get("data")
                        .and_then(|data| data.get("location"))
                        .and_then(|location| location.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or(&location.name);
                    result(
                        CAT_TRAVEL,
                        "猫猫旅行",
                        "done",
                        format!("猫猫已出发前往「{name}」，数小时后归来"),
                        0,
                    )
                }
                Ok(value) => {
                    let msg = value.get("msg").and_then(Value::as_str).unwrap_or("上游未说明原因");
                    result(CAT_TRAVEL, "猫猫旅行", "failed", format!("派出被拒：{msg}"), 0)
                }
                Err(error) => result(CAT_TRAVEL, "猫猫旅行", "failed", error.describe(), 0),
            }
        }
        // 剩下两种都不打网络，交给纯函数（可单测）。
        other => travel_static_outcome(other),
    }
}

/// 不需要联网就能定的旅行结果。
///
/// `traveling` 必须是 `skipped` 而不是 `failed`：它表示"猫猫还没回来"，
/// 既不是成功也不是失败。记成 `failed` 会让一键执行长期挂着假的失败计数，
/// 用户就无法从红点里分辨"真掉号"和"等它回家"。
fn travel_static_outcome(state: &str) -> TaskRunResult {
    match state {
        "traveling" => result(
            CAT_TRAVEL,
            "猫猫旅行",
            "skipped",
            "猫猫正在旅行途中，稍后再来查看".into(),
            0,
        ),
        other => result(
            CAT_TRAVEL,
            "猫猫旅行",
            "failed",
            format!("状态 {other} 不知道该做什么"),
            0,
        ),
    }
}

/// 目的地清单 → 取第一个。查不到就退回 `id = 1`（参考实现同口径）。
async fn travel_location(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> TravelLocation {
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Cn,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    let found = transport::get_json(client, &hosts.chat_url(TRAVEL_CONFIG_PATH), &headers)
        .await
        .ok()
        .and_then(|payload| payload.get("data").cloned())
        .and_then(|data| data.get("locations").cloned())
        .and_then(|locations| locations.as_array().and_then(|list| list.first().cloned()));
    match found {
        Some(location) => TravelLocation {
            id: location.get("id").cloned().unwrap_or(json!(1)),
            name: location
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("未知目的地")
                .to_string(),
        },
        None => TravelLocation {
            id: json!(1),
            name: "未知目的地".to_string(),
        },
    }
}

struct TravelLocation {
    id: Value,
    name: String,
}

// ── 国际版：每日活跃对话 ─────────────────────────────────────────────────

fn chat_action(_account: &AccountSnapshot) -> DailyAction {
    // 上游没有"今天聊没聊"的查询接口（参考实现是本地记 lastDailyChat），
    // 所以这里**不断言已完成**：宁可多聊一句（对话本来就没副作用），也别漏掉积分。
    DailyAction {
        code: DAILY_CHAT.into(),
        name: "每日活跃对话".into(),
        action: "去对话".into(),
        done_today: false,
        note: "每天一条即可（送积分）；没有查询接口，所以这里不判断是否已做".into(),
        automatable: true,
    }
}

async fn daily_chat(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> TaskRunResult {
    // 国际版用国际站的出站身份（与对话一致），模型固定一个确定存在的。
    let headers = identity::chat_headers(
        &account.channel_id,
        Realm::Intl,
        Outbound::Desktop,
        hosts,
        account,
        1,
    );
    let body = json!({
        "model": DAILY_CHAT_MODEL,
        "messages": [{ "role": "user", "content": "Hi" }],
        "stream": false,
        "max_tokens": 10,
    });
    match transport::post_json(client, &hosts.chat_url("/v1/chat/completions"), &headers, &body)
        .await
    {
        Ok(value) if value.get("code").and_then(Value::as_i64) == Some(0) => result(
            DAILY_CHAT,
            "每日活跃对话",
            "done",
            "已发出一条对话".into(),
            0,
        ),
        Ok(value) => {
            let msg = value.get("msg").and_then(Value::as_str).unwrap_or("上游未说明原因");
            result(
                DAILY_CHAT,
                "每日活跃对话",
                "failed",
                format!("上游拒绝：{msg}"),
                0,
            )
        }
        Err(error) => result(DAILY_CHAT, "每日活跃对话", "failed", error.describe(), 0),
    }
}

// ── 小工具 ────────────────────────────────────────────────────────────────

fn daily_name(code: &str) -> &'static str {
    match code {
        SIGN_IN => "每日签到",
        CAT_TRAVEL => "猫猫旅行",
        DAILY_CHAT => "每日活跃对话",
        _ => "每日动作",
    }
}

fn result(code: &str, name: &str, status: &str, message: String, earned: u32) -> TaskRunResult {
    TaskRunResult {
        code: code.to_string(),
        name: name.to_string(),
        status: status.to_string(),
        message,
        earned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 动作按版本分开：国际版不能拿到国内版的签到/猫猫，反之亦然。
    /// （这里只测"名字映射与未知组合"这两段纯逻辑，网络部分靠真机探针。）
    #[test]
    fn realm_specific_codes_are_named() {
        assert_eq!(daily_name(SIGN_IN), "每日签到");
        assert_eq!(daily_name(CAT_TRAVEL), "猫猫旅行");
        assert_eq!(daily_name(DAILY_CHAT), "每日活跃对话");
        assert_eq!(daily_name("nope"), "每日动作");
    }

    /// SPEC 14 M6：「traveling 的猫猫旅行记为 skipped 而非 failed」。
    /// 这条断言是红点可信度的根 —— 一旦回退成 failed，一键执行的失败数
    /// 就会永久带一条假阳性，用户再也分不出真掉号。
    #[test]
    fn traveling_counts_as_skipped_not_failed() {
        let outcome = travel_static_outcome("traveling");
        assert_eq!(outcome.code, CAT_TRAVEL);
        assert_eq!(outcome.status, "skipped");
        assert_eq!(outcome.earned, 0);
    }

    /// 未知状态才是真的"不知道该做什么"，仍然记 failed。
    #[test]
    fn unknown_travel_state_stays_failed() {
        let outcome = travel_static_outcome("weird");
        assert_eq!(outcome.status, "failed");
        assert!(outcome.message.contains("weird"));
    }
}
