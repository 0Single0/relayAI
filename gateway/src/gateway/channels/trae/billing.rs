//! Trae 额度（积分）与每日签到。
//!
//! 两个接口都在用户中心（`api.trae.cn`），鉴权仍是 `Cloud-IDE-JWT`：
//! - `POST /trae/api/v2/pay/ide_user_ent_usage` — 通用积分余额
//! - `POST /trae/api/v2/ug/checkin_credits/{status,claim}` — 每日签到
//!
//! 签到有两个必须尊重的约束（都来自实测）：
//! ① **`9074` 不是"重试就好"**：短时间内重复 claim 会把风控窗口越拉越长，
//!    所以拿到 9074 就停手，返回"已被限流"，而不是立刻再打一次；
//! ② 已经签过要算**成功**（幂等），否则每日任务会天天报红。

use serde_json::{json, Value};

use crate::gateway::{
    channels::{AccountSnapshot, QuotaSnapshot},
    errors::GatewayError,
    timeutil::now_ms,
    transport,
};

use super::identity::{self, Hosts};

const ENT_USAGE_PATH: &str = "/trae/api/v2/pay/ide_user_ent_usage";
const CHECKIN_STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CHECKIN_CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";

/// 上游风控：短时间内别重复 claim。
pub const RATE_LIMITED_CODE: i64 = 9074;
/// 今天已经签过（幂等成功）。
pub const ALREADY_CHECKED_CODE: i64 = 10001;

/// 用户中心接口的头组：比对话头轻，只带鉴权与设备号。
fn ug_headers(account: &AccountSnapshot) -> Vec<(String, String)> {
    vec![
        identity::auth_header(account),
        ("Content-Type".into(), "application/json".into()),
        ("x-device-id".into(), identity::device_id(account)),
    ]
}

fn ug_url(hosts: &Hosts, path: &str) -> String {
    format!("{}{path}", hosts.ug.trim_end_matches('/'))
}

/// 查通用积分余额。
///
/// `req_source` 是**整数**：1 = 通用 IDE 积分，2 = Work 专项。
/// 传字符串（`"ide"`）会被上游当成非法参数 —— 这是"额度刷新不了"的
/// 另一条常见原因，别改回字符串。
///
/// 关键约定：**业务失败要报错，不能静默返回 None**。
/// `None` 的语义是"这个渠道没有额度口径"，面板会显示"无额度"；
/// 而凭据过期/接口变动导致查不到时，必须让用户看到具体原因，
/// 否则"刷新额度"点了没反应，完全没法排查。
pub async fn fetch_quota(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Result<Option<QuotaSnapshot>, GatewayError> {
    let payload = transport::post_json(
        client,
        &ug_url(hosts, ENT_USAGE_PATH),
        &ug_headers(account),
        &json!({"require_usage": true, "req_source": 1}),
    )
    .await?;

    if let Some(code) = payload.get("code").and_then(Value::as_i64) {
        if code != 0 {
            let message = payload
                .get("message")
                .or_else(|| payload.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // 401/凭据类 → 让保鲜链路去刷新；其余按上游异常报出来。
            return Err(match code {
                401 | 1001 | 1002 | 1003 => GatewayError::new(
                    crate::gateway::errors::ErrorKind::AuthExpired,
                    crate::gateway::errors::ErrorScope::Account,
                    401,
                    format!("额度接口拒绝凭据（code={code}）：{message}"),
                    true,
                ),
                _ => GatewayError::upstream(
                    502,
                    if message.is_empty() {
                        format!("额度接口返回 code={code}")
                    } else {
                        format!("额度接口返回 code={code}：{message}")
                    },
                ),
            }
            .with_upstream(payload.to_string()));
        }
    }

    match parse_quota(&payload) {
        Some(snapshot) => Ok(Some(snapshot)),
        None => {
            // code=0 但一个额度包都没有：接口形状变了，或这个账号确实没有额度。
            // 把原始响应的形状提示出来，而不是让面板静默显示"无额度"。
            Err(GatewayError::upstream(
                502,
                "额度响应里没有解析到任何额度包（user_entitlement_pack_list 为空或字段已变更）",
            )
            .with_upstream(crate::gateway::channels::openai_compat::raw(
                &payload.to_string(),
            )))
        }
    }
}

/// 把上游的用量响应转成渠道无关的额度快照。
///
/// **真实响应结构**（与直觉完全不同，别按字段名猜）：
/// ```json
/// { "code": 0,
///   "user_entitlement_pack_list": [
///     { "entitlement_base_info": {
///         "available_endpoint": 0,
///         "quota": { "credits_limit": 500 } },
///       "usage": { "credits_amount": 123.5 } } ],
///   "is_credits_billing": true }
/// ```
/// 要点：
/// ① 总额/已用是**跨 pack 求和**，不是某个顶层字段；
/// ② `product_type=0` 的功能包没有 `credits_limit`，必须跳过（否则把功能包当额度）；
/// ③ `credits_limit == -1` 表示无限；
/// ④ `available_endpoint`：0 = 通用 IDE 积分，1 = Work 专项 —— 账号页只该看通用那部分，
///    但工作区在用 Work 时又需要它，所以两个都汇总、分开列在详情里。
pub fn parse_quota(payload: &Value) -> Option<QuotaSnapshot> {
    // 业务错误：HTTP 200 + code≠0 也是失败，不能当成"额度 0"。
    if let Some(code) = payload.get("code").and_then(Value::as_i64) {
        if code != 0 {
            return None;
        }
    }
    let data = payload
        .get("data")
        .or_else(|| payload.get("result"))
        .unwrap_or(payload);

    let packs = data
        .get("user_entitlement_pack_list")
        .or_else(|| data.get("userEntitlementPackList"))
        .and_then(Value::as_array);

    let (mut total, mut used, mut unlimited) = (0.0f64, 0.0f64, false);
    let (mut work_total, mut work_used) = (0.0f64, 0.0f64);
    let mut counted = 0usize;
    for pack in packs.into_iter().flatten() {
        let base = pack
            .get("entitlement_base_info")
            .or_else(|| pack.get("entitlementBaseInfo"))
            .cloned()
            .unwrap_or(Value::Null);
        let quota = base.get("quota").cloned().unwrap_or(Value::Null);
        // 没有 credits_limit 的是功能包（product_type=0），不算额度。
        let Some(limit) = number_of(quota.get("credits_limit")) else {
            continue;
        };
        let amount = number_of(
            pack.get("usage")
                .and_then(|usage| usage.get("credits_amount")),
        )
        .unwrap_or(0.0);
        let is_work = base
            .get("available_endpoint")
            .and_then(Value::as_i64)
            .map(|endpoint| endpoint == 1)
            .unwrap_or(false);
        counted += 1;
        if limit < 0.0 {
            unlimited = true;
        } else {
            total += limit;
            if is_work {
                work_total += limit;
            }
        }
        if amount >= 0.0 {
            used += amount;
            if is_work {
                work_used += amount;
            }
        }
    }

    // 一个 pack 都没解析出来：回落"扁平字段"的旧口径（少数账期版本是平的）。
    if counted == 0 {
        return parse_flat_quota(data);
    }

    let remaining = if unlimited {
        None
    } else {
        Some((total - used).max(0.0))
    };
    let mut details = Vec::new();
    if unlimited {
        details.push(("额度类型".to_string(), "无限".to_string()));
    }
    details.push(("额度包".to_string(), format!("{counted} 个")));
    if work_total > 0.0 || work_used > 0.0 {
        // Work 专项额度是独立的池子，混进总数会让人以为通用积分被吃了。
        details.push((
            "Work 专项".to_string(),
            format!("剩 {:.2} / 总 {:.2}", (work_total - work_used).max(0.0), work_total),
        ));
    }
    if let Some(flag) = data.get("is_credits_billing").and_then(Value::as_bool) {
        details.push((
            "计费方式".to_string(),
            if flag { "积分" } else { "订阅" }.to_string(),
        ));
    }
    Some(QuotaSnapshot {
        label: "积分".to_string(),
        unit: "点".to_string(),
        remaining,
        total: (!unlimited).then_some(total),
        used: Some(used),
        details,
        fetched_at_ms: now_ms(),
    })
}

/// 扁平字段的兜底（少数账期版本把额度直接放在顶层）。
fn parse_flat_quota(data: &Value) -> Option<QuotaSnapshot> {
    let number = |candidates: &[&str]| -> Option<f64> {
        candidates
            .iter()
            .find_map(|key| number_of(data.get(*key)))
    };
    let remaining = number(&["remaining", "RemainCredit", "remain_credit", "remain", "balance"]);
    let total = number(&["total_limit", "TotalCredit", "total", "total_credits"]);
    let used = number(&["used", "UsedCredit", "used_credit"]);
    if remaining.is_none() && total.is_none() && used.is_none() {
        return None;
    }
    Some(QuotaSnapshot {
        label: "积分".to_string(),
        unit: "点".to_string(),
        remaining,
        total,
        used,
        details: Vec::new(),
        fetched_at_ms: now_ms(),
    })
}

/// 数字或数字字符串 → f64（上游大量用字符串传小数）。
fn number_of(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// 签到结果：`Ok(true)` = 今天签到成功或本来就已经签过。
///
/// 失败一律折成 `false`（而不是 Err）：签到只是运营动作，
/// 不该让面板的一次点击弹出红色异常。
pub async fn check_in(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> bool {
    // 先查状态：已经签过就别再打 claim（那会把风控窗口拉长）。
    if let Ok(status) = transport::post_json(
        client,
        &ug_url(hosts, CHECKIN_STATUS_PATH),
        &ug_headers(account),
        &json!({}),
    )
    .await
    {
        if is_already_checked(&status) {
            return true;
        }
    }
    match transport::post_json(
        client,
        &ug_url(hosts, CHECKIN_CLAIM_PATH),
        &ug_headers(account),
        &json!({}),
    )
    .await
    {
        Ok(payload) => {
            if is_rate_limited(&payload) {
                // 9074 = 风控窗口。**不再重试**：连续 claim 会把窗口越拉越长
                // （参考实现踩过这个坑），如实返回失败让定时任务明天再来。
                eprintln!(
                    "[gateway] Trae 签到触发风控（9074），本次不再重试账号 {}",
                    account.uid
                );
                return false;
            }
            let code = payload.get("code").and_then(Value::as_i64).unwrap_or(0);
            matches!(code, 0 | ALREADY_CHECKED_CODE)
        }
        Err(_) => false,
    }
}

/// 状态响应里的"今天已经签过"判定。
pub fn is_already_checked(payload: &Value) -> bool {
    let data = payload.get("data").unwrap_or(payload);
    for key in ["checked_in", "checkedIn", "already_checked", "alreadyChecked"] {
        if let Some(flag) = data.get(key).and_then(Value::as_bool) {
            return flag;
        }
    }
    // 有些版本用 `status` 字符串表达。
    if let Some(status) = data.get("status").and_then(Value::as_str) {
        if matches!(
            status.to_ascii_lowercase().as_str(),
            "checked" | "done" | "claimed" | "already"
        ) {
            return true;
        }
    }
    // 再退一步：`code` 是幂等码也算签过。
    payload.get("code").and_then(Value::as_i64) == Some(ALREADY_CHECKED_CODE)
}

/// 上游是否处于风控窗口（面板据此显示"稍后再试"而不是"签到失败"）。
pub fn is_rate_limited(payload: &Value) -> bool {
    payload.get("code").and_then(Value::as_i64) == Some(RATE_LIMITED_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应结构：额度在 `user_entitlement_pack_list[]` 里，要跨包求和。
    #[test]
    fn quota_sums_entitlement_packs_and_skips_feature_packs() {
        let snapshot = parse_quota(&json!({
            "code": 0,
            "is_credits_billing": true,
            "user_entitlement_pack_list": [
                // 真额度包：500 总额 / 已用 123.5。
                {
                    "entitlement_base_info": {"available_endpoint": 0, "quota": {"credits_limit": 500}},
                    "usage": {"credits_amount": "123.5"}
                },
                // 第二个通用包：再 200。
                {
                    "entitlement_base_info": {"available_endpoint": 0, "quota": {"credits_limit": "200"}},
                    "usage": {"credits_amount": 0}
                },
                // 功能包（没有 credits_limit）：必须跳过，不能当额度。
                {
                    "entitlement_base_info": {"available_endpoint": 0},
                    "usage": {"credits_amount": 0}
                }
            ]
        }))
        .expect("应当解析出额度");
        assert_eq!(snapshot.total, Some(700.0), "跨包求和");
        assert_eq!(snapshot.used, Some(123.5), "字符串小数也要认");
        assert_eq!(snapshot.remaining, Some(576.5));
        assert_eq!(snapshot.label, "积分");
        assert!(snapshot
            .details
            .iter()
            .any(|(key, value)| key == "额度包" && value == "2 个"));
        assert!(snapshot
            .details
            .iter()
            .any(|(key, value)| key == "计费方式" && value == "积分"));
    }

    /// Work 专项额度（available_endpoint=1）单独列出，不能和通用积分混在一起。
    #[test]
    fn work_packs_are_reported_separately() {
        let snapshot = parse_quota(&json!({
            "code": 0,
            "user_entitlement_pack_list": [
                {"entitlement_base_info": {"available_endpoint": 0, "quota": {"credits_limit": 100}},
                 "usage": {"credits_amount": 10}},
                {"entitlement_base_info": {"available_endpoint": 1, "quota": {"credits_limit": 50}},
                 "usage": {"credits_amount": 5}}
            ]
        }))
        .expect("应当解析");
        assert_eq!(snapshot.total, Some(150.0), "总数含 Work");
        assert!(
            snapshot
                .details
                .iter()
                .any(|(key, _)| key == "Work 专项"),
            "Work 额度要单独列一行"
        );
    }

    /// `credits_limit = -1` 表示无限：剩余量给 None 而不是编一个 0。
    #[test]
    fn unlimited_pack_reports_no_remaining() {
        let snapshot = parse_quota(&json!({
            "code": 0,
            "user_entitlement_pack_list": [
                {"entitlement_base_info": {"quota": {"credits_limit": -1}}, "usage": {}}
            ]
        }))
        .expect("应当解析");
        assert_eq!(snapshot.remaining, None);
        assert_eq!(snapshot.total, None);
        assert!(snapshot
            .details
            .iter()
            .any(|(key, value)| key == "额度类型" && value == "无限"));
    }

    /// 业务错误必须能从响应里认出来（面板据此报"凭据失效"而不是"无额度"）。
    #[test]
    fn business_error_response_is_not_mistaken_for_zero_quota() {
        // 401 类：凭据问题，面板要提示重新授权。
        assert!(parse_quota(&json!({"code": 401, "message": "token expired"})).is_none());
        // 额度接口返回错误码时也不能当成"额度 0"。
        assert!(parse_quota(&json!({"code": 500, "message": "boom"})).is_none());
        // 但 code=0 + 正常 pack 要能解析（别把成功响应当失败）。
        assert!(parse_quota(&json!({
            "code": 0,
            "user_entitlement_pack_list": [
                {"entitlement_base_info": {"quota": {"credits_limit": 10}}, "usage": {}}
            ]
        }))
        .is_some());
    }

    #[test]
    fn flat_fields_still_work_as_a_fallback() {
        let snapshot = parse_quota(&json!({
            "data": {"remaining": "12.50", "total_limit": 100, "used": 87.5}
        }))
        .expect("扁平字段兜底");
        assert_eq!(snapshot.remaining, Some(12.5));
        assert_eq!(snapshot.total, Some(100.0));
    }

    #[test]
    fn quota_without_any_number_is_none_not_zero() {
        // "查不到"与"额度是 0"是两回事：前者不该显示成 0。
        assert!(parse_quota(&json!({"data": {}})).is_none());
        // 业务错误也不算额度。
        assert!(parse_quota(&json!({"code": 401, "user_entitlement_pack_list": []})).is_none());
    }

    #[test]
    fn already_checked_is_recognised_in_all_shapes() {
        assert!(is_already_checked(&json!({"data": {"checked_in": true}})));
        assert!(is_already_checked(&json!({"data": {"status": "claimed"}})));
        assert!(is_already_checked(&json!({"code": ALREADY_CHECKED_CODE})));
        assert!(!is_already_checked(&json!({"data": {"checked_in": false}})));
        assert!(!is_already_checked(&json!({"data": {}})));
    }

    #[test]
    fn risk_window_is_identified() {
        assert!(is_rate_limited(&json!({"code": RATE_LIMITED_CODE})));
        assert!(!is_rate_limited(&json!({"code": 0})));
    }
}
