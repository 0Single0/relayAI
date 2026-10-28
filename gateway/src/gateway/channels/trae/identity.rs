//! Trae 出站身份：上游主机、请求头组与 `common_params`。
//!
//! Trae Remote 是**网页版**协议：上游按 `X-Trae-Client-Type`、`Origin`、`x-user-region`
//! 判断你是哪一端。这些字段必须成套出现，单独改一个会被当成异常客户端
//! （表现为空响应或 `2001`/`4023` 一类的业务码，而不是清晰的 401）。

use serde_json::Value;

use crate::gateway::{channels::AccountSnapshot, security};

/// 网页版 Trae Remote 网关（含 `/api/remote/v1` 版本段）。
///
/// 与 TraeCode IDE/Solo 的主机（只到主机名、路径由 relay 追加）**刻意分开**：
/// 两者的基址拼法不同，混用会拼出 `/api/remote/v1/api/ide/...` 这种 404 路径。
pub const DEFAULT_BASE_URL: &str = "https://trae-api-cn.mchost.guru/api/remote/v1";
/// 网页版来源；`Origin`/`Referer` 都用它。
pub const DEFAULT_ORIGIN: &str = "https://solo.trae.cn";
/// 兑换 Cloud-IDE-JWT 的用户中心主机。
pub const DEFAULT_UG_API_HOST: &str = "https://api.trae.cn";
/// 网页授权页。
pub const AUTH_URL: &str = "https://www.trae.cn/authorization";
/// Trae 网页端公开的 client_id（授权页参数，非密钥）。
pub const CLIENT_ID: &str = "ono9krqynydwx5";

/// 网页版 UA：上游对 UA 不敏感，但对"完全不像浏览器"的值会走另一条风控分支。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// 由渠道 `options` 决定的上游地址。
///
/// 放在渠道配置而不是账号凭据里：企业网关是**部署级**差异，
/// 同一渠道下的账号本来就该共用同一个上游。
#[derive(Debug, Clone)]
pub struct Hosts {
    /// Remote 网关基址（已含 `/api/remote/v1`）。
    pub remote: String,
    /// 用户中心（ExchangeToken / GetUserInfo）。
    pub ug: String,
}

impl Hosts {
    pub fn from_options(options: &Value) -> Self {
        let remote = options
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        // 只填了主机名的部署（企业网关常见）补齐版本段，否则所有
        // chat_sessions/models 请求都会 404。
        let remote = if remote.contains("/api/") {
            remote
        } else {
            format!("{remote}/api/remote/v1")
        };
        let ug = options
            .get("ug_base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_UG_API_HOST.to_string());
        Self { remote, ug }
    }

    pub fn chat_sessions_url(&self) -> String {
        format!("{}/chat_sessions", self.remote)
    }

    pub fn events_url(&self, session_id: &str, reply_to_message_id: &str) -> String {
        format!(
            "{}/chat_sessions/{}/events?reply_to_message_id={}",
            self.remote,
            urlencode(session_id),
            urlencode(reply_to_message_id)
        )
    }

    pub fn models_url(&self) -> String {
        format!(
            "{}/models?functions=solo_agent_remote%2Csolo_work_remote%2Csolo_design_remote&show_custom_model=true",
            self.remote
        )
    }

    /// 原生模型目录：**元数据（上下文/输出/档位）只在这里**。
    ///
    /// 注意基址不同：它挂在主机根的 `/api/ide/v1/` 下，
    /// 不在 `/api/remote/v1` 版本段里 —— 拼错就是 404。
    pub fn detail_param_url(&self) -> String {
        format!("{}/api/ide/v1/get_detail_param", self.host())
    }

    /// 从 `remote` 里剥出主机根（去掉 `/api/...` 之后的部分）。
    fn host(&self) -> &str {
        self.remote.split("/api/").next().unwrap_or(&self.remote)
    }

    pub fn exchange_token_url(&self) -> String {
        format!("{}/cloudide/api/v3/trae/oauth/ExchangeToken", self.ug)
    }
}

/// 极简 URL 编码：只处理 path segment 里会出现的那几个字符。
///
/// 不引入 percent-encoding 依赖：会话 id / 消息 id 都是字母数字，
/// 真出现特殊字符时按字节转义即可。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 鉴权头：Trae 用 `Cloud-IDE-JWT <token>`，**不是** `Bearer`。
pub fn auth_header(account: &AccountSnapshot) -> (String, String) {
    (
        "Authorization".into(),
        format!(
            "Cloud-IDE-JWT {}",
            account.access_token.as_deref().unwrap_or_default()
        ),
    )
}

/// 对话/目录接口的公共头。
pub fn base_headers(account: &AccountSnapshot, accept: &str) -> Vec<(String, String)> {
    vec![
        auth_header(account),
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), accept.to_string()),
        ("X-Trae-Client-Type".into(), "web".into()),
        ("X-Preferenced-Language".into(), "zh-CN".into()),
        ("x-user-region".into(), "CN".into()),
        ("Origin".into(), DEFAULT_ORIGIN.into()),
        ("Referer".into(), format!("{DEFAULT_ORIGIN}/")),
        ("User-Agent".into(), USER_AGENT.into()),
    ]
}

/// 原生目录（`get_detail_param`）的专用头。
///
/// 这个接口走 **TraeCode 客户端**的身份，不是网页身份。版本号必须成套且
/// 用真实的客户端值：上游按 `x-ide-version` / `x-ide-version-code` /
/// `package-type` 判断能不能拿到完整目录。随便填一个（比如 `1.0.0`）
/// 会得到 **200 + 空 `config_info_list`** —— 表现就是"模型都在，
/// 但输出上限与思考档位全空"，而 HTTP 层面完全看不出错误。
///
/// 这些常量取自参考实现的实测值，改动前请确认上游仍接受。
pub fn detail_headers(account: &AccountSnapshot) -> Vec<(String, String)> {
    const GATEWAY_VERSION: &str = "0.1.52";
    const GATEWAY_VERSION_CODE: &str = "20260811";
    const DEVICE_BRAND: &str = "83DG";
    const DEVICE_TYPE: &str = "windows";
    const PLATFORM: &str = "windows";
    const PACKAGE_TYPE: &str = "stable_cn";
    const OS_VERSION: &str = "Windows 11 Pro";
    /// 上游渠道标识（Solo 客户端的埋点 id）。
    const LSCBD_AID: &str = "787976";

    let mut headers = base_headers(account, "application/json");
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("User-Agent"));
    headers.extend([
        ("User-Agent".into(), format!("Trae/{GATEWAY_VERSION}")),
        ("x-app-version".into(), "default".into()),
        ("x-app-version-code".into(), GATEWAY_VERSION_CODE.into()),
        ("x-ide-version".into(), GATEWAY_VERSION.into()),
        ("x-ide-version-code".into(), GATEWAY_VERSION_CODE.into()),
        ("x-ide-version-type".into(), "stable".into()),
        ("x-version-code".into(), GATEWAY_VERSION_CODE.into()),
        ("x-device-brand".into(), DEVICE_BRAND.into()),
        ("x-device-type".into(), DEVICE_TYPE.into()),
        ("x-device-platform".into(), PLATFORM.into()),
        ("x-device-id".into(), device_id(account)),
        ("x-os-version".into(), OS_VERSION.into()),
        ("package-type".into(), PACKAGE_TYPE.into()),
        ("x-plugin-channel".into(), "icube-ai".into()),
        ("app-version".into(), GATEWAY_VERSION.into()),
        ("request-traffic-type".into(), "prod".into()),
        ("x-lscbd-aid".into(), LSCBD_AID.into()),
        ("x-lscbd-platform".into(), PLATFORM.into()),
        ("x-ss-dp".into(), LSCBD_AID.into()),
    ]);
    headers
}

/// 稳定派生的 19 位数字设备号（Trae 网页端用 19 位随机数字）。
///
/// 由 (渠道, uid) 派生而不是每次随机：同一账号在 Trae 眼里必须是同一台设备，
/// 否则签到/风控会把它当成新设备（对应参考实现的 `9074` 换设备重试）。
pub fn device_id(account: &AccountSnapshot) -> String {
    let seed = security::derive_id(&account.channel_id, &account.uid, "trae-device");
    let mut digits = String::with_capacity(19);
    for byte in seed.bytes() {
        if let Some(value) = (byte as char).to_digit(16) {
            digits.push(char::from(b'0' + (value % 10) as u8));
            if digits.len() == 19 {
                break;
            }
        }
    }
    while digits.len() < 19 {
        digits.push('0');
    }
    digits
}

/// Remote 会话的 `common_params`（JSON **字符串**，上游要求就是字符串）。
pub fn common_params(account: &AccountSnapshot, mode: &str, session_id: &str) -> String {
    let device = device_id(account);
    let machine = security::derive_id(&account.channel_id, &account.uid, "trae-machine");
    let mut params = serde_json::json!({
        "language": "zh-cn",
        "app_language": "zh-CN",
        "quality": "stable",
        "app_version": "1.0.0.1229",
        "web_id": account.uid,
        "user_identity": "Free",
        "is_freshman": "0",
        "biz_user_id": account.uid,
        "user_unique_id": account.uid,
        "scope": "marscode-cn",
        "tenant": "marscode",
        "region": "cn",
        "aiRegion": "cn",
        "is_privacy_mode": 0,
        "privacy_mode": "off",
        "solo_chat_mode": mode,
    });
    if let Some(object) = params.as_object_mut() {
        if mode.eq_ignore_ascii_case("work") {
            object.insert("icube_uid".into(), account.uid.clone().into());
            object.insert("user_id".into(), account.uid.clone().into());
            object.insert("device_id".into(), device.into());
            object.insert("local_device_id".into(), format!("aha-{machine}").into());
            object.insert("machine_id".into(), machine.into());
            object.insert("arch".into(), "x64".into());
            object.insert("system".into(), "windows".into());
            object.insert("build_version".into(), "2.3.79946".into());
            object.insert("vscode_version".into(), "1.107.1".into());
            object.insert("app_version".into(), "3.3.96".into());
            object.insert("platform".into(), "electron".into());
            object.insert("device_model".into(), "TraeCode Windows".into());
            object.insert("device_manufacturer".into(), "TraeCode".into());
            object.insert("product_code".into(), "SOLO_Lite".into());
        }
        if !session_id.is_empty() {
            object.insert("biz_session_id".into(), session_id.into());
        }
    }
    params.to_string()
}

/// 模型名归一：Trae 的上游网关**大小写敏感**地匹配 `config_name`，
/// 而对外发布的 id 一律小写。已知别名在这里翻译，未知的交给目录精确匹配。
pub fn normalize_model(model: &str) -> String {
    let trimmed = model.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "auto" | "" => "auto".to_string(),
        "work" | "auto-work" => "work".to_string(),
        _ => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account() -> AccountSnapshot {
        AccountSnapshot {
            channel_id: "trae-cn".into(),
            uid: "uid-1".into(),
            access_token: Some("jwt".into()),
            refresh_token: Some("rt".into()),
            enterprise_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn base_url_gets_the_version_segment_exactly_once() {
        let plain = json!({ "base_url": "https://trae-api-cn.mchost.guru" });
        assert_eq!(
            Hosts::from_options(&plain).remote,
            "https://trae-api-cn.mchost.guru/api/remote/v1"
        );

        let full = json!({ "base_url": "https://trae-api-cn.mchost.guru/api/remote/v1/" });
        assert_eq!(
            Hosts::from_options(&full).remote,
            "https://trae-api-cn.mchost.guru/api/remote/v1"
        );

        // 非 http(s) 的脏配置回落默认值，不能拼出 reqwest 运行期才炸的 URL。
        let junk = json!({ "base_url": "trae-api-cn.mchost.guru" });
        assert_eq!(Hosts::from_options(&junk).remote, DEFAULT_BASE_URL);
        assert_eq!(Hosts::from_options(&json!({})).remote, DEFAULT_BASE_URL);
    }

    #[test]
    fn auth_uses_cloud_ide_jwt_scheme() {
        let headers = base_headers(&account(), "application/json");
        let auth = headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.clone());
        assert_eq!(auth.as_deref(), Some("Cloud-IDE-JWT jwt"));
    }

    #[test]
    fn device_id_is_stable_nineteen_digits_and_channel_scoped() {
        let first = device_id(&account());
        assert_eq!(first, device_id(&account()), "同账号必须恒定");
        assert_eq!(first.len(), 19, "必须补到 19 位");
        assert!(first.bytes().all(|byte| byte.is_ascii_digit()));

        let mut other = account();
        other.uid = "uid-2".into();
        assert_ne!(first, device_id(&other), "换账号必须不同");
    }

    #[test]
    fn common_params_are_a_json_string_with_the_mode() {
        let text = common_params(&account(), "code", "sess-1");
        let parsed: Value = serde_json::from_str(&text).expect("必须是 JSON 字符串");
        assert_eq!(parsed["solo_chat_mode"], "code");
        assert_eq!(parsed["biz_session_id"], "sess-1");
        assert_eq!(parsed["scope"], "marscode-cn");

        // work 档要额外带设备与产品字段（上游按这些判断 Work 会话）。
        let work: Value = serde_json::from_str(&common_params(&account(), "work", "")).expect("JSON");
        assert_eq!(work["product_code"], "SOLO_Lite");
        assert_eq!(work["platform"], "electron");
    }

    #[test]
    fn events_url_escapes_segments() {
        let hosts = Hosts::from_options(&json!({}));
        let url = hosts.events_url("a/b", "m 1");
        assert!(url.ends_with("/chat_sessions/a%2Fb/events?reply_to_message_id=m%201"));
    }
}
