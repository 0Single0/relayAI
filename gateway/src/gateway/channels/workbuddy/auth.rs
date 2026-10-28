//! WorkBuddy 登录、刷新与 JWT 解析。
//!
//! 三条容易踩的规则（都来自实测）：
//! ① **没有本地回调**：浏览器在上游站内完成授权，我们只轮询 `auth/token?state=`；
//! ② uid 不在响应里，要解 JWT 的 `sub`；区域判定看 `iss`；
//! ③ refresh_token 会轮换，所以刷新**必须单账号串行**（由调度层持锁，见 `core::refresh`）。

use base64::Engine as _;
use serde_json::{json, Value};

use crate::gateway::{
    channels::{AccountSnapshot, Credential, LoginPoll, LoginTicket},
    errors::GatewayError,
    transport,
};

use super::identity::{self, Hosts};

pub const AUTH_STATE_PATH: &str = "/v2/plugin/auth/state";
pub const AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token";
pub const REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";
pub const LOGIN_ACCOUNT_PATH: &str = "/v2/plugin/login/account";
pub const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";

/// 上游表示"用户尚未完成授权"的码。
pub const PENDING_CODE: i64 = 11217;
/// 幂等成功码：今天已签到。
pub const ALREADY_DONE_CODE: i64 = 10001;

/// 申请一次授权：拿到 state 与给用户浏览器打开的链接。
pub async fn login_start(
    client: &reqwest::Client,
    hosts: &Hosts,
    platform: &str,
) -> Result<LoginTicket, GatewayError> {
    let url = format!("{}?platform={platform}", hosts.chat_url(AUTH_STATE_PATH));
    let payload = transport::post_json(client, &url, &identity::base_headers(hosts), &json!({})).await?;
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let state = data
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .ok_or_else(|| GatewayError::upstream(502, "授权接口没有返回 state"))?;
    let auth_url = data
        .get("authUrl")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| GatewayError::upstream(502, "授权接口没有返回 authUrl"))?;
    Ok(LoginTicket {
        state: state.to_string(),
        auth_url: auth_url.to_string(),
    })
}

/// 轮询一次授权结果。**每次都真实打上游**（上游才记着授权状态，我们不做中间缓存）。
pub async fn login_poll(
    client: &reqwest::Client,
    hosts: &Hosts,
    state: &str,
) -> Result<LoginPoll, GatewayError> {
    let url = format!("{}?state={state}", hosts.chat_url(AUTH_TOKEN_PATH));
    let payload = match transport::get_json(client, &url, &identity::base_headers(hosts)).await {
        Ok(payload) => payload,
        // 轮询期间的网络抖动不该让面板报错：当作"还没好"。
        Err(error) if error.kind == crate::gateway::errors::ErrorKind::Network => {
            return Ok(LoginPoll::Pending)
        }
        Err(error) => return Err(error),
    };
    Ok(match parse_token_response(&payload) {
        // 成功时顺手取昵称：这个接口只在登录窗口内可用，失败不影响凭据可用。
        LoginPoll::Ready(mut credential) => {
            if let Some(name) =
                nickname(client, hosts, state, &credential.access_token).await
            {
                credential.nickname = Some(name);
            }
            LoginPoll::Ready(credential)
        }
        other => other,
    })
}

/// `auth/token` 的响应语义：11217 = 还在等授权；0 且带 accessToken = 成功。
pub fn parse_token_response(payload: &Value) -> LoginPoll {
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == PENDING_CODE {
        return LoginPoll::Pending;
    }
    if code != 0 {
        return LoginPoll::Failed(format!(
            "code={code} msg={}",
            payload.get("msg").and_then(Value::as_str).unwrap_or("")
        ));
    }
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let credential = credential_from_data(&data);
    if credential.access_token.is_empty() {
        // code=0 但没有 token：上游还没准备好，继续等。
        return LoginPoll::Pending;
    }
    LoginPoll::Ready(Box::new(credential))
}

/// 刷新响应可能是 `data` 也可能是 `data.data` 双层嵌套，两种都要兼容。
pub fn parse_refresh_response(payload: &Value) -> Option<Credential> {
    let data = payload.get("data")?;
    let data = data.get("data").unwrap_or(data);
    let credential = credential_from_data(data);
    (!credential.access_token.is_empty()).then_some(credential)
}

/// 把上游凭据块转成内部结构：uid / 企业身份 / 过期时间兜底都从 JWT 里解。
pub fn credential_from_data(data: &Value) -> Credential {
    let access_token = data
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let claims = parse_jwt(&access_token);
    Credential {
        access_token,
        refresh_token: data
            .get("refreshToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        // 上游给的是毫秒时间戳；缺了就用 JWT 的 exp 兜底。
        expires_at_ms: data
            .get("expiresAt")
            .and_then(Value::as_u64)
            .or_else(|| claims.exp_secs.map(|secs| secs * 1000)),
        uid: claims.sub.clone(),
        nickname: None,
        enterprise_id: claims.enterprise_id.clone(),
        domain: data
            .get("domain")
            .and_then(Value::as_str)
            .map(str::to_string),
        // CodeArts 那套 AK/SK 字段只在华为云渠道用得上，这里留空。
        ..Default::default()
    }
}

/// 用 state 换昵称。只在登录窗口内有效，失败可忽略（退化成 uid 前 8 位）。
pub async fn nickname(
    client: &reqwest::Client,
    hosts: &Hosts,
    state: &str,
    access_token: &str,
) -> Option<String> {
    let url = format!("{}?state={state}", hosts.chat_url(LOGIN_ACCOUNT_PATH));
    let mut headers = identity::base_headers(hosts);
    headers.push((
        "Authorization".into(),
        format!("Bearer {access_token}"),
    ));
    let payload = transport::get_json(client, &url, &headers).await.ok()?;
    payload
        .get("data")
        .and_then(|data| data.get("nickname"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// 刷新凭据（调用方负责串行化）。
pub async fn refresh(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
    refresh_token: &str,
) -> Result<Credential, GatewayError> {
    let mut headers = identity::base_headers(hosts);
    headers.extend([
        ("X-Refresh-Token".into(), refresh_token.to_string()),
        // 国内版专属值。
        ("X-Auth-Refresh-Source".into(), "workbuddy".into()),
        ("X-User-Id".into(), account.uid.clone()),
        ("X-Domain".into(), hosts.domain.clone()),
        ("X-CodeBuddy-Request".into(), "1".into()),
        ("Accept-Language".into(), "zh-CN".into()),
    ]);
    if let Some(enterprise) = account.enterprise_id.as_deref().filter(|id| !id.is_empty()) {
        headers.push(("X-Enterprise-Id".into(), enterprise.to_string()));
    }

    let payload =
        transport::post_json(client, &hosts.chat_url(REFRESH_PATH), &headers, &json!({})).await?;
    let mut credential = parse_refresh_response(&payload)
        .ok_or_else(|| GatewayError::new(
            crate::gateway::errors::ErrorKind::AuthExpired,
            crate::gateway::errors::ErrorScope::Account,
            401,
            "刷新没有拿到新凭据，需要重新登录",
            false,
        ))?;
    // 上游偶尔不回 refreshToken：沿用旧的，别把账号刷新能力丢了。
    if credential.refresh_token.is_none() {
        credential.refresh_token = Some(refresh_token.to_string());
    }
    if credential.uid.is_none() {
        credential.uid = Some(account.uid.clone());
    }
    Ok(credential)
}

/// 每日签到（国内版积分）。返回是否算成功（含幂等）。
pub async fn sign_in(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> bool {
    let headers = identity::billing_headers(hosts, account, None);
    match transport::post_json(
        client,
        &hosts.billing_url(CHECKIN_PATH),
        &headers,
        &json!({}),
    )
    .await
    {
        Ok(payload) => {
            let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
            code == 0 || code == ALREADY_DONE_CODE
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JwtClaims {
    pub sub: Option<String>,
    pub exp_secs: Option<u64>,
    pub enterprise_id: Option<String>,
}

/// JWT 只做 base64url 解码取字段，**不验签**（我们不持有上游公钥，也没这个必要）。
pub fn parse_jwt(token: &str) -> JwtClaims {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), _) = (parts.next(), parts.next(), parts.next()) else {
        return JwtClaims::default();
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return JwtClaims::default();
    };
    let Ok(claims) = serde_json::from_slice::<Value>(&bytes) else {
        return JwtClaims::default();
    };
    JwtClaims {
        sub: claims
            .get("sub")
            .and_then(|value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .filter(|sub| !sub.is_empty()),
        exp_secs: claims.get("exp").and_then(Value::as_u64),
        enterprise_id: claims
            .get("enterprise_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个只有 payload 有意义的 JWT（不验签，所以签名段随便填）。
    fn jwt(payload: Value) -> String {
        let encode = |value: Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
        };
        format!(
            "{}.{}.{}",
            encode(json!({"alg": "HS256"})),
            encode(payload),
            "sig"
        )
    }

    #[test]
    fn jwt_fields_are_read_without_verification() {
        let token = jwt(json!({
            "sub": "uid-123",
            "exp": 1_750_000_000,
            "iss": "https://copilot.tencent.com",
            "enterprise_id": "ent-7"
        }));
        let claims = parse_jwt(&token);
        assert_eq!(claims.sub.as_deref(), Some("uid-123"));
        assert_eq!(claims.exp_secs, Some(1_750_000_000));
        assert_eq!(claims.enterprise_id.as_deref(), Some("ent-7"));
        assert_eq!(parse_jwt("not-a-jwt"), JwtClaims::default());
    }

    #[test]
    fn pending_code_is_pending_not_failure() {
        assert!(matches!(
            parse_token_response(&json!({"code": PENDING_CODE, "msg": "waiting"})),
            LoginPoll::Pending
        ));
        // code=0 但没有 token：继续等，不当失败。
        assert!(matches!(
            parse_token_response(&json!({"code": 0, "data": {}})),
            LoginPoll::Pending
        ));
        assert!(matches!(
            parse_token_response(&json!({"code": 12345, "msg": "boom"})),
            LoginPoll::Failed(_)
        ));
    }

    #[test]
    fn successful_poll_yields_uid_and_expiry_from_jwt() {
        let access = jwt(json!({"sub": "uid-9", "exp": 1_800_000_000}));
        let poll = parse_token_response(&json!({
            "code": 0,
            "data": {"accessToken": access, "refreshToken": "rt", "domain": "copilot.tencent.com"}
        }));
        let LoginPoll::Ready(credential) = poll else {
            panic!("应当拿到凭据");
        };
        assert_eq!(credential.uid.as_deref(), Some("uid-9"));
        assert_eq!(credential.expires_at_ms, Some(1_800_000_000_000));
        assert_eq!(credential.refresh_token.as_deref(), Some("rt"));
    }

    #[test]
    fn refresh_response_accepts_nested_data() {
        let access = jwt(json!({"sub": "uid-9", "exp": 1_900_000_000}));
        let flat = parse_refresh_response(&json!({"data": {"accessToken": access}}));
        assert_eq!(flat.map(|c| c.access_token.is_empty()), Some(false));
        let nested = parse_refresh_response(&json!({"data": {"data": {"accessToken": access}}}));
        assert!(nested.is_some(), "双层嵌套必须兼容");
        assert!(parse_refresh_response(&json!({"data": {}})).is_none());
    }
}
