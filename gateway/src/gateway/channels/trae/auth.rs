//! Trae 认证：网页授权、`ExchangeToken` 兑换与凭据刷新。
//!
//! 与 workbuddy 的三点关键差异：
//! ① 授权回调**必须**是 `http://127.0.0.1:<port>/authorize`（Trae 硬性要求），
//!    而网关是 Tauri 应用、没有常驻的本地 HTTP 监听，所以这里改为
//!    **让用户粘贴回调 URL**（面板里的「手动填写凭据」）；
//! ② Trae 的 access token 是 Cloud-IDE-JWT，过期后用 refreshToken 走
//!    `oauth/ExchangeToken` 换新的（不是 REST 的 refresh 端点）；
//! ③ 上游从不在响应里给 uid，要从 userInfo 里取，取不到就用 refreshToken 的
//!    稳定哈希兜底 —— 账号总得有个 id 才能进池。

use base64::Engine as _;
use serde_json::{json, Value};

use crate::gateway::{
    channels::{AccountSnapshot, Credential, LoginPoll, LoginTicket},
    errors::{ErrorKind, ErrorScope, GatewayError},
    transport,
};

use super::identity::{self, CLIENT_ID};

/// 一次待完成的授权：state 里自带 trace id，回调时用它校验。
pub const LOGIN_STATE_PREFIX: &str = "trae:";

/// 生成授权链接。
///
/// 没有本地回调端口，`auth_callback_url` 仍按 Trae 要求写成回环地址 ——
/// 用户在该页完成后浏览器会跳到那个地址（打不开），把地址栏整条复制回来即可。
pub fn login_start(platform: &str) -> LoginTicket {
    let trace = uuid::Uuid::new_v4().simple().to_string();
    let machine = uuid::Uuid::new_v4().simple().to_string();
    let device: String = trace.chars().filter(char::is_ascii_digit).collect();
    let device = format!("{device:0<19}").chars().take(19).collect::<String>();
    let params = [
        ("login_version", "1"),
        ("auth_from", "solo"),
        ("login_channel", "native_ide"),
        ("plugin_version", "2.3.24254"),
        ("auth_type", "local"),
        ("client_id", CLIENT_ID),
        ("redirect", "0"),
        ("login_trace_id", &trace),
        (
            "auth_callback_url",
            "http://127.0.0.1:8765/authorize",
        ),
        ("machine_id", &machine),
        ("device_id", &device),
        ("x_device_id", &device),
        ("x_machine_id", &machine),
        ("x_device_brand", platform),
        ("x_device_type", "windows"),
        ("x_os_version", "Windows 11"),
        ("x_env", ""),
        ("x_app_version", "3.3.65"),
        ("x_app_type", "stable"),
        ("hide_saas_login", "true"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", key, identity::urlencode(value)))
        .collect::<Vec<_>>()
        .join("&");
    LoginTicket {
        state: format!("{LOGIN_STATE_PREFIX}{trace}"),
        auth_url: format!("{}?{query}", identity::AUTH_URL),
    }
}

/// Trae 的网页授权**没有**可轮询的 token 端点：用户必须把回调 URL 交回来。
///
/// 这里如实返回 `Pending` 而不是伪造失败 —— 面板据此显示"等待粘贴回调"，
/// 真正的入库走 [`super::channel`] 的 `import_callback`。
pub fn login_poll() -> LoginPoll {
    LoginPoll::Pending
}

/// 把用户粘贴的回调 URL（或其查询串）解析成凭据。
///
/// 兼容两种历史流程：
/// ① 新流程：`userJwt` + `userInfo` 两个 JSON 参数；
/// ② 老流程：只有 `refreshToken`，需要再走一次 ExchangeToken。
pub async fn credential_from_callback(
    client: &reqwest::Client,
    hosts: &identity::Hosts,
    callback: &str,
) -> Result<Credential, GatewayError> {
    let query = parse_query(callback);
    let user_jwt = query
        .iter()
        .find(|(key, _)| key == "userJwt")
        .map(|(_, value)| parse_json(value))
        .unwrap_or(Value::Null);
    let user_info = query
        .iter()
        .find(|(key, _)| key == "userInfo")
        .map(|(_, value)| parse_json(value))
        .unwrap_or(Value::Null);

    let mut token = pick(&user_jwt, &["Token", "token"])
        .or_else(|| {
            query
                .iter()
                .find(|(key, _)| key == "token")
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_default();
    let mut refresh = pick(&user_jwt, &["RefreshToken", "refreshToken"])
        .or_else(|| {
            query
                .iter()
                .find(|(key, _)| key == "refreshToken")
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_default();
    let mut expires_at_ms = pick(&user_jwt, &["TokenExpireAt", "tokenExpireAt"])
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(normalize_expiry);
    let mut uid = pick(&user_info, &["UserID", "userId", "userID"]).unwrap_or_default();
    let nickname = pick(&user_info, &["ScreenName", "screenName", "nickName"]);

    // 老流程：只有 refreshToken。
    if token.is_empty() && !refresh.is_empty() {
        let exchanged = exchange(client, hosts, &refresh).await?;
        token = exchanged.token;
        if !exchanged.refresh.is_empty() {
            refresh = exchanged.refresh;
        }
        expires_at_ms = exchanged.expires_at_ms.or(expires_at_ms);
        if uid.is_empty() {
            uid = exchanged.uid;
        }
    }

    if token.is_empty() {
        return Err(GatewayError::invalid_request(
            "回调里没有 token：请确认复制的是授权完成后浏览器地址栏里的**完整** URL",
        ));
    }
    // uid 兜底：JWT 的 sub 优先，其次 userInfo，最后用 refreshToken 的稳定哈希。
    if uid.is_empty() {
        uid = jwt_subject(&token)
            .or_else(|| (!refresh.is_empty()).then(|| stable_uid(&refresh)))
            .unwrap_or_else(|| stable_uid(&token));
    }
    if expires_at_ms.is_none() {
        expires_at_ms = jwt_expiry_ms(&token);
    }
    Ok(Credential {
        access_token: token,
        refresh_token: (!refresh.is_empty()).then_some(refresh),
        expires_at_ms,
        uid: Some(uid),
        nickname,
        enterprise_id: None,
        // 端点由渠道 options 决定（部署级），不跟账号走；这里不重复存。
        domain: None,
        ..Default::default()
    })
}

/// 用 refresh token 换新的 Cloud-IDE-JWT。
pub async fn refresh(
    client: &reqwest::Client,
    hosts: &identity::Hosts,
    _account: &AccountSnapshot,
    refresh_token: &str,
) -> Result<Credential, GatewayError> {
    let exchanged = exchange(client, hosts, refresh_token).await?;
    if exchanged.token.is_empty() {
        return Err(GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            "兑换接口没有返回新的 Cloud-IDE-JWT，需要重新授权",
            false,
        ));
    }
    Ok(Credential {
        access_token: exchanged.token,
        // 上游偶尔不回新的 refreshToken：沿用旧的，别把账号的刷新能力丢掉。
        refresh_token: Some(if exchanged.refresh.is_empty() {
            refresh_token.to_string()
        } else {
            exchanged.refresh
        }),
        expires_at_ms: exchanged.expires_at_ms,
        uid: (!exchanged.uid.is_empty()).then_some(exchanged.uid),
        nickname: None,
        enterprise_id: None,
        domain: None,
        ..Default::default()
    })
}

struct Exchanged {
    token: String,
    refresh: String,
    uid: String,
    expires_at_ms: Option<u64>,
}

/// `POST /cloudide/api/v3/trae/oauth/ExchangeToken`。
async fn exchange(
    client: &reqwest::Client,
    hosts: &identity::Hosts,
    refresh_token: &str,
) -> Result<Exchanged, GatewayError> {
    let payload = json!({
        "ClientID": CLIENT_ID,
        "RefreshToken": refresh_token,
        "ClientSecret": "-",
        "UserID": "",
    });
    let response = transport::post_json(
        client,
        &hosts.exchange_token_url(),
        &[("Content-Type".into(), "application/json".into())],
        &payload,
    )
    .await?;
    let data = response
        .get("Result")
        .or_else(|| response.get("result"))
        .unwrap_or(&response);
    let token = pick(data, &["Token", "token", "AccessToken", "accessToken"]).unwrap_or_default();
    if token.is_empty() {
        return Err(GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            "兑换接口没有返回 Token（refresh token 可能已失效）",
            false,
        ));
    }
    let user_info = pick(data, &["UserInfo", "userInfo"])
        .as_deref()
        .map(parse_json)
        .unwrap_or(Value::Null);
    let uid = pick(&user_info, &["UserID", "userId", "userID"])
        .or_else(|| jwt_subject(&token))
        .unwrap_or_default();
    let expires_at_ms = pick(data, &["TokenExpireAt", "tokenExpireAt", "expiredAt"])
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(normalize_expiry)
        .or_else(|| jwt_expiry_ms(&token));
    Ok(Exchanged {
        token,
        refresh: pick(data, &["RefreshToken", "refreshToken"]).unwrap_or_default(),
        uid,
        expires_at_ms,
    })
}

fn pick(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
}

fn parse_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// 回调 URL 或裸查询串 → 键值对（已 URL 解码）。
pub fn parse_query(input: &str) -> Vec<(String, String)> {
    let trimmed = input.trim();
    let query = match trimmed.split_once('?') {
        Some((_, rest)) => rest,
        None => trimmed,
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 上游给的时间戳有秒有毫秒，统一成毫秒。
fn normalize_expiry(value: i64) -> Option<u64> {
    if value <= 0 {
        return None;
    }
    Some(if value < 10_000_000_000 {
        (value as u64) * 1000
    } else {
        value as u64
    })
}

/// 不验签地取 JWT 的 `sub` / `exp`：我们不持有上游公钥，也没这个必要。
fn jwt_claims(token: &str) -> Value {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), _) = (parts.next(), parts.next(), parts.next()) else {
        return Value::Null;
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return Value::Null;
    };
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn jwt_subject(token: &str) -> Option<String> {
    let claims = jwt_claims(token);
    ["sub", "user_id", "userId"]
        .iter()
        .find_map(|key| claims.get(*key))
        .and_then(|value| match value {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
}

fn jwt_expiry_ms(token: &str) -> Option<u64> {
    jwt_claims(token)
        .get("exp")
        .and_then(Value::as_i64)
        .and_then(normalize_expiry)
}

/// 没有 uid 时的稳定兜底 id：同一份凭据永远得到同一个 uid。
fn stable_uid(seed: &str) -> String {
    crate::gateway::security::derive_id("trae", seed, "uid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: Value) -> String {
        let encode = |value: Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        };
        format!(
            "{}.{}.{}",
            encode(json!({"alg": "RS256"})),
            encode(payload),
            "sig"
        )
    }

    #[test]
    fn login_ticket_carries_a_loopback_callback_and_trace() {
        let ticket = login_start("Trae");
        assert!(ticket.state.starts_with(LOGIN_STATE_PREFIX));
        assert!(ticket.auth_url.starts_with("https://www.trae.cn/authorization?"));
        // Trae 硬性要求回环回调。
        assert!(ticket.auth_url.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A8765%2Fauthorize"));
        assert!(ticket.auth_url.contains("client_id=ono9krqynydwx5"));
    }

    #[test]
    fn query_parsing_handles_full_urls_encoding_and_bare_strings() {
        let full = "http://127.0.0.1:8765/authorize?userJwt=%7B%22Token%22%3A%22t%22%7D&scope=a+b";
        let parsed = parse_query(full);
        assert_eq!(
            parsed.iter().find(|(key, _)| key == "userJwt").map(|(_, v)| v.as_str()),
            Some("{\"Token\":\"t\"}")
        );
        assert_eq!(
            parsed.iter().find(|(key, _)| key == "scope").map(|(_, v)| v.as_str()),
            Some("a b"),
            "加号要解码成空格"
        );
        // 裸查询串也要能解析。
        let bare = parse_query("refreshToken=rt&userId=u1");
        assert_eq!(bare.len(), 2);
    }

    #[test]
    fn jwt_fields_are_read_without_verification() {
        let token = jwt(json!({"sub": "uid-9", "exp": 1_800_000_000}));
        assert_eq!(jwt_subject(&token).as_deref(), Some("uid-9"));
        assert_eq!(jwt_expiry_ms(&token), Some(1_800_000_000_000));
        assert!(jwt_subject("not-a-jwt").is_none());
    }

    #[test]
    fn expiry_accepts_seconds_and_milliseconds() {
        assert_eq!(normalize_expiry(1_800_000_000), Some(1_800_000_000_000));
        assert_eq!(normalize_expiry(1_800_000_000_000), Some(1_800_000_000_000));
        assert_eq!(normalize_expiry(0), None);
    }

    #[test]
    fn stable_uid_is_deterministic() {
        assert_eq!(stable_uid("rt-1"), stable_uid("rt-1"));
        assert_ne!(stable_uid("rt-1"), stable_uid("rt-2"));
    }

    /// 新流程：回调里直接带 userJwt，不需要任何网络请求。
    #[tokio::test]
    async fn new_flow_callback_yields_credential_without_network() {
        let hosts = identity::Hosts::from_options(&json!({}));
        let client = reqwest::Client::new();
        let token = jwt(json!({"sub": "uid-777", "exp": 1_900_000_000}));
        let user_jwt = json!({"Token": token, "RefreshToken": "rt-9"}).to_string();
        let user_info = json!({"UserID": "uid-777", "ScreenName": "小明"}).to_string();
        let callback = format!(
            "http://127.0.0.1:8765/authorize?userJwt={}&userInfo={}",
            identity::urlencode(&user_jwt),
            identity::urlencode(&user_info)
        );
        let credential = credential_from_callback(&client, &hosts, &callback)
            .await
            .expect("应当解析出凭据");
        assert_eq!(credential.uid.as_deref(), Some("uid-777"));
        assert_eq!(credential.nickname.as_deref(), Some("小明"));
        assert_eq!(credential.refresh_token.as_deref(), Some("rt-9"));
        assert_eq!(credential.expires_at_ms, Some(1_900_000_000_000));
        assert_eq!(
            credential.access_token, token,
            "回调里的 userJwt.Token 要原样落库"
        );
    }

    /// 回调里什么都没有：必须给可读的报错，而不是静默入库一个空账号。
    #[tokio::test]
    async fn callback_without_token_is_rejected_with_a_readable_message() {
        let hosts = identity::Hosts::from_options(&json!({}));
        let error = credential_from_callback(&reqwest::Client::new(), &hosts, "http://x/authorize?scope=a")
            .await
            .expect_err("没有 token 必须报错");
        assert!(error.message.contains("完整"), "{}", error.message);
    }
}
