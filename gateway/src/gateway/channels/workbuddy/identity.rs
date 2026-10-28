//! WorkBuddy 出站身份：域名、UA、请求头组。
//!
//! 三个身份（桌面端 / 官方 CLI / 官方 VSCode 插件）× 两个区域（国内版 / 国际版）。
//! 上游用 UA 与头组判断"你是什么客户端"，所以身份切换时**端点、X-Domain、UA 必须同步换**，
//! 否则会 401/403 或拿到不完整的模型目录。

use crate::gateway::{channels::AccountSnapshot, security};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realm {
    /// 国内版：copilot.tencent.com + www.codebuddy.cn
    Cn,
    /// 国际版：www.workbuddy.ai
    Intl,
}

impl Realm {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "intl" | "international" | "global" => Self::Intl,
            _ => Self::Cn,
        }
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outbound {
    /// 桌面端（默认）：UA `WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1`
    Desktop,
    /// 官方 CodeBuddy CLI。
    Cli,
    /// 官方 VSCode 插件（走 www.workbuddy.cn）。
    Vscode,
}

impl Outbound {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "cli" | "codebuddy" => Self::Cli,
            "vscode" | "vsc" => Self::Vscode,
            _ => Self::Desktop,
        }
    }

}

#[derive(Debug, Clone)]
pub struct Hosts {
    /// 对话与配置端点。
    pub chat: String,
    /// 计费/签到端点。
    pub billing: String,
    /// Origin/Referer 用。
    pub origin: String,
    /// `X-Domain` 的值。
    pub domain: String,
}

impl Hosts {
    pub fn chat_url(&self, path: &str) -> String {
        format!("{}://{}{}", self.scheme(), self.chat, path)
    }

    pub fn billing_url(&self, path: &str) -> String {
        format!("{}://{}{}", self.scheme(), self.billing, path)
    }

    fn scheme(&self) -> &'static str {
        "https"
    }
}

pub fn hosts(realm: Realm, outbound: Outbound) -> Hosts {
    match (realm, outbound) {
        (Realm::Cn, Outbound::Vscode) => Hosts {
            chat: "www.workbuddy.cn".into(),
            billing: "www.codebuddy.cn".into(),
            origin: "https://www.codebuddy.cn".into(),
            domain: "www.workbuddy.cn".into(),
        },
        (Realm::Cn, _) => Hosts {
            chat: "copilot.tencent.com".into(),
            billing: "www.codebuddy.cn".into(),
            origin: "https://www.codebuddy.cn".into(),
            domain: "copilot.tencent.com".into(),
        },
        (Realm::Intl, Outbound::Vscode) => Hosts {
            chat: "www.workbuddy.cn".into(),
            billing: "www.workbuddy.ai".into(),
            origin: "https://www.workbuddy.ai".into(),
            domain: "www.workbuddy.ai".into(),
        },
        (Realm::Intl, _) => Hosts {
            chat: "www.workbuddy.ai".into(),
            billing: "www.workbuddy.ai".into(),
            origin: "https://www.workbuddy.ai".into(),
            domain: "www.workbuddy.ai".into(),
        },
    }
}

fn user_agent(realm: Realm, outbound: Outbound) -> String {
    match (realm, outbound) {
        (Realm::Cn, Outbound::Desktop) => {
            "WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1".into()
        }
        (Realm::Cn, Outbound::Cli) => "CLI/2.117.2 CodeBuddy/2.117.2".into(),
        (Realm::Cn, Outbound::Vscode) => "VSCode/1.96.0 WorkBuddy/5.5.6".into(),
        (Realm::Intl, Outbound::Desktop) => {
            "WorkBuddy/5.5.2 WorkBuddy AI/5.5.2 CLI/5.5.2".into()
        }
        (Realm::Intl, Outbound::Cli) => "CLI/2.117.2 CodeBuddy/2.117.2".into(),
        (Realm::Intl, Outbound::Vscode) => "VSCode/1.96.0 WorkBuddy/5.5.2".into(),
    }
}

/// 覆盖 `User-Agent`（而不是追加两个）：`/v3/config` 必须用桌面长 UA，
/// 否则上游直接 400（code 12403）。
pub fn with_user_agent(mut headers: Vec<(String, String)>, user_agent: &str) -> Vec<(String, String)> {
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("User-Agent"));
    headers.push(("User-Agent".into(), user_agent.to_string()));
    headers
}

/// 桌面身份的长 UA（目录接口专用）。
pub fn desktop_user_agent(realm: Realm) -> String {
    user_agent(realm, Outbound::Desktop)
}

/// 授权类接口的固定头（不带鉴权）：申请 state、轮询 token 都用这一套。
///
/// 注意：`User-Agent` 是计费/授权口径的短 UA，**不要**换成桌面长 UA，
/// 只有 `/v3/config` 才必须用桌面长 UA。
pub fn base_headers(hosts: &Hosts) -> Vec<(String, String)> {
    vec![
        ("Content-Type".into(), "application/json".into()),
        (
            "Accept".into(),
            "application/json, text/plain, */*".into(),
        ),
        ("X-Requested-With".into(), "XMLHttpRequest".into()),
        ("User-Agent".into(), billing_user_agent(&hosts.domain)),
        ("Origin".into(), hosts.origin.clone()),
        ("Referer".into(), format!("{}/", hosts.origin)),
    ]
}

fn billing_user_agent(domain: &str) -> String {
    if domain.contains("workbuddy.ai") {
        "WorkBuddy/5.5.2".into()
    } else {
        "WorkBuddy/5.5.6".into()
    }
}

/// 会话身份头（官方桌面端的 `X-Conversation-*` 家族）。
///
/// **缺这一族会直接 `11-128 Illegal API invocation from an unapproved channel`**
/// —— 参考实现在 `wb_identity.py` 里明确记了这个成因（"conversation id missing"）。
/// 官方客户端每一轮对话都带齐这四个 id，缺任何一个都像"非官方调用"。
///
/// 四个 id 的语义（与官方客户端对齐）：
/// | 头 | 生命周期 | 形态 |
/// |---|---|---|
/// | `X-Conversation-ID` | 整个会话恒定 | 带连字符的 UUID |
/// | `X-Conversation-Request-ID` | 一轮 user send | 32 位无连字符 hex |
/// | `X-Root-Request-ID` | 同上一轮（两者等值） | 同上 |
/// | `X-Conversation-Message-ID` | 每个请求 | 同上 |
/// | `X-Request-ID` | 同 Message-ID | 同上 |
///
/// 上游**不拿这些 id 存历史**（实测换 id 回答不变），所以稳定派生只为"像官方"，
/// 不承担语义职责。
pub fn conversation_headers(
    channel_id: &str,
    uid: &str,
    session_key: Option<&str>,
) -> Vec<(String, String)> {
    // 会话 id：有会话键就按 (渠道, uid, 会话键) 稳定派生，没有就每次新生成。
    let conversation_id = match session_key.filter(|key| !key.trim().is_empty()) {
        Some(key) => stable_uuid(channel_id, uid, key),
        None => uuid::Uuid::new_v4().to_string(),
    };
    // 一轮请求 id 与逐请求 message id（32 位无连字符，与官方一致的形态）。
    let turn_request_id = message_id();
    let message = message_id();
    let span = message_id()[..16].to_string();
    let mut headers = vec![
        ("X-Conversation-ID".to_string(), conversation_id),
        (
            "X-Conversation-Request-ID".to_string(),
            turn_request_id.clone(),
        ),
        ("X-Root-Request-ID".to_string(), turn_request_id),
        (
            "X-Conversation-Message-ID".to_string(),
            message.clone(),
        ),
        ("X-Request-ID".to_string(), message.clone()),
        // 官方主对话固定是 main；子代理才用别的值。
        ("X-Agent-Type".to_string(), "main".to_string()),
        // 国内站不发这个头，国际站默认发 —— 统一带上，语义是"允许私有数据"。
        ("X-Private-Data".to_string(), "true".to_string()),
    ];
    // 链路追踪头：官方每条 chat 请求都带，缺了同样像非官方调用。
    headers.extend([
        ("X-Trace-ID".to_string(), message.clone()),
        (
            "traceparent".to_string(),
            format!("00-{message}-{span}-01"),
        ),
        ("b3".to_string(), format!("{message}-{span}-1")),
        ("X-B3-TraceId".to_string(), message),
        ("X-B3-SpanId".to_string(), span),
    ]);
    headers
}

/// 32 位无连字符的小写 hex（官方 `X-Request-ID` 的形态）。
fn message_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 由 (渠道, uid, 会话键) 稳定派生的 UUID（带连字符、v4 版本位）。
fn stable_uuid(channel_id: &str, uid: &str, session_key: &str) -> String {
    let seed = security::derive_id(
        channel_id,
        uid,
        &format!("conversation:{session_key}"),
    );
    // derive_id 给 36 位 hex；补足到 32 再按 UUID 排布。
    let mut hex: String = seed.chars().take(32).collect();
    while hex.len() < 32 {
        hex.push('0');
    }
    let bytes: Vec<char> = hex.chars().collect();
    // 版本位（第 13 个 hex）固定 4，变体位（第 17 个）固定 a —— 与参考实现一致。
    let mut formatted: Vec<char> = bytes.clone();
    formatted[12] = '4';
    formatted[16] = 'a';
    let text: String = formatted.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &text[0..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..32]
    )
}

/// 把会话身份头并进已有的头组，**并替换**掉旧的 `X-Request-ID`。
///
/// 必须替换而不是追加：`reqwest` 的 `.header()` 是追加语义，
/// 同时发两个 `X-Request-ID` 比不发更像异常客户端。
pub fn with_conversation_identity(
    mut headers: Vec<(String, String)>,
    channel_id: &str,
    uid: &str,
    session_key: Option<&str>,
) -> Vec<(String, String)> {
    headers.retain(|(name, _)| {
        !name.eq_ignore_ascii_case("x-request-id")
            && !name.eq_ignore_ascii_case("x-conversation-id")
            && !name.eq_ignore_ascii_case("x-agent-type")
            && !name.eq_ignore_ascii_case("x-private-data")
    });
    headers.extend(conversation_headers(channel_id, uid, session_key));
    headers
}

/// 对话与配置请求的头组：鉴权 + 设备指纹 + 出站身份。
pub fn chat_headers(
    channel_id: &str,
    realm: Realm,
    outbound: Outbound,
    hosts: &Hosts,
    account: &AccountSnapshot,
    request_sequence: u64,
) -> Vec<(String, String)> {
    // 对话身份用长 UA：必须**替换**基头里的短 UA，否则会同时发两个 User-Agent。
    let mut headers = with_user_agent(base_headers(hosts), &user_agent(realm, outbound));
    let uid = account.uid.as_str();
    headers.extend([
        (
            "Authorization".into(),
            format!(
                "Bearer {}",
                account.access_token.as_deref().unwrap_or_default()
            ),
        ),
        ("X-User-Id".into(), uid.to_string()),
        ("X-Domain".into(), hosts.domain.clone()),
        ("X-CodeBuddy-Request".into(), "1".into()),
        ("Accept-Language".into(), "zh-CN".into()),
        // 指纹：由 (渠道, uid) 稳定派生，多账号之间天然隔离。
        (
            "X-Machine-ID".into(),
            security::derive_id(channel_id, uid, "machine"),
        ),
        (
            "X-Session-ID".into(),
            security::derive_id(channel_id, uid, "session"),
        ),
        (
            "X-Request-ID".into(),
            security::request_id(channel_id, uid, request_sequence),
        ),
    ]);

    match outbound {
        Outbound::Desktop => {
            // 与官方桌面端抓包逐项对齐：`X-Product` 是 **SaaS**（不是产品名），
            // 且必须带 `X-Agent-Intent: craft` —— 少这个上游会判成"非官方调用"。
            headers.extend([
                ("X-Agent-Intent".into(), "craft".into()),
                ("X-Agent-Purpose".into(), "conversation".into()),
                ("X-Agent-Type".into(), "main".into()),
                ("X-IDE-Name".into(), "WorkBuddy".into()),
                ("X-IDE-Type".into(), "WorkBuddy".into()),
                ("X-IDE-Version".into(), "5.5.6".into()),
                ("X-Product".into(), "SaaS".into()),
                ("X-Private-Data".into(), "true".into()),
            ]);
        }
        Outbound::Cli => {
            headers.extend([
                ("X-Agent-Purpose".into(), "conversation".into()),
                ("X-Agent-Intent".into(), "craft".into()),
                ("X-IDE-Name".into(), "CodeBuddy".into()),
                ("X-IDE-Type".into(), "CLI".into()),
                ("X-Product".into(), "SaaS".into()),
            ]);
        }
        Outbound::Vscode => {
            headers.extend([
                ("X-IDE-Name".into(), "VSCode".into()),
                ("X-IDE-Type".into(), "VSCode".into()),
                ("X-Product".into(), "SaaS".into()),
                ("X-Env-ID".into(), "production".into()),
            ]);
        }
    }

    if let Some(enterprise) = account.enterprise_id.as_deref() {
        if !enterprise.is_empty() {
            headers.push(("X-Enterprise-Id".into(), enterprise.to_string()));
            headers.push(("X-Tenant-Id".into(), enterprise.to_string()));
        }
    }
    headers
}

/// 计费侧头组：比对话头更轻，无企业身份时显式声明 `X-No-Enterprise-Id: 1`。
pub fn billing_headers(
    hosts: &Hosts,
    account: &AccountSnapshot,
    client_platform: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = base_headers(hosts);
    headers.push((
        "Authorization".into(),
        format!(
            "Bearer {}",
            account.access_token.as_deref().unwrap_or_default()
        ),
    ));
    headers.push(("X-User-Id".into(), account.uid.clone()));
    headers.push(("X-Domain".into(), hosts.domain.clone()));
    headers.push(("X-CodeBuddy-Request".into(), "1".into()));
    headers.push(("X-Product".into(), "SaaS".into()));
    if let Some(platform) = client_platform {
        headers.push(("X-Client-Platform".into(), platform.to_string()));
    }
    headers.push((
        "X-Machine-ID".into(),
        security::derive_id(&account.channel_id, &account.uid, "machine"),
    ));
    headers.push((
        "X-Session-ID".into(),
        security::derive_id(&account.channel_id, &account.uid, "session"),
    ));
    match account.enterprise_id.as_deref() {
        Some(enterprise) if !enterprise.is_empty() => {
            headers.push(("X-Enterprise-Id".into(), enterprise.to_string()));
        }
        _ => headers.push(("X-No-Enterprise-Id".into(), "1".into())),
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account() -> AccountSnapshot {
        AccountSnapshot {
            channel_id: "wb-cn".into(),
            uid: "uid-1".into(),
            access_token: Some("token".into()),
            refresh_token: Some("refresh".into()),
            enterprise_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn identity_switch_changes_endpoint_domain_and_user_agent() {
        let desktop = hosts(Realm::Cn, Outbound::Desktop);
        let vscode = hosts(Realm::Cn, Outbound::Vscode);
        assert_eq!(desktop.chat, "copilot.tencent.com");
        assert_eq!(vscode.chat, "www.workbuddy.cn");

        let headers = chat_headers("wb-cn", Realm::Cn, Outbound::Desktop, &desktop, &account(), 1);
        let find = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(find("X-Domain").as_deref(), Some("copilot.tencent.com"));
        assert_eq!(
            find("User-Agent").as_deref(),
            Some("WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1")
        );
        assert_eq!(find("X-IDE-Type").as_deref(), Some("WorkBuddy"));
        assert_eq!(find("Authorization").as_deref(), Some("Bearer token"));

        let cli = chat_headers("wb-cn", Realm::Cn, Outbound::Cli, &desktop, &account(), 1);
        assert!(cli
            .iter()
            .any(|(key, value)| key == "X-IDE-Type" && value == "CLI"));
    }

    /// `X-Conversation-*` 族是**防止 11-128 的必需项**：缺了上游直接判
    /// "conversation id missing"。这里锁住"一个都不能少"。
    #[test]
    fn conversation_identity_sends_the_whole_family() {
        let headers = conversation_headers("wb-cn", "uid-1", Some("sess-1"));
        let find = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        for required in [
            "X-Conversation-ID",
            "X-Conversation-Request-ID",
            "X-Root-Request-ID",
            "X-Conversation-Message-ID",
            "X-Request-ID",
            "X-Trace-ID",
            "traceparent",
            "b3",
            "X-B3-TraceId",
            "X-B3-SpanId",
        ] {
            assert!(find(required).is_some(), "缺少必需头 {required}");
        }
        assert_eq!(find("X-Agent-Type").as_deref(), Some("main"));
        assert_eq!(find("X-Private-Data").as_deref(), Some("true"));
        // 一轮内 Request-ID 与 Root-Request-ID 等值，Message-ID 独立。
        assert_eq!(
            find("X-Conversation-Request-ID"),
            find("X-Root-Request-ID"),
            "同一轮的 turn id 要一致"
        );
        assert_eq!(find("X-Request-ID"), find("X-Conversation-Message-ID"));
    }

    /// 形态要对：Conversation-ID 带连字符，其余 32 位无连字符。
    #[test]
    fn conversation_ids_have_the_official_shape() {
        let headers = conversation_headers("wb-cn", "uid-1", Some("sess-1"));
        let find = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        let conversation = find("X-Conversation-ID");
        assert_eq!(conversation.len(), 36, "UUID 带连字符应 36 位：{conversation}");
        assert_eq!(conversation.matches('-').count(), 4);
        for name in ["X-Request-ID", "X-Conversation-Message-ID"] {
            let value = find(name);
            assert_eq!(value.len(), 32, "{name} 应 32 位：{value}");
            assert!(
                value.chars().all(|ch| ch.is_ascii_hexdigit()),
                "{name} 应为 hex：{value}"
            );
        }
    }

    /// 同一会话必须复用同一个 Conversation-ID（多轮对话不该每次都换）。
    #[test]
    fn conversation_id_is_stable_within_a_session() {
        let first = conversation_headers("wb-cn", "uid-1", Some("sess-1"));
        let second = conversation_headers("wb-cn", "uid-1", Some("sess-1"));
        let pick = |headers: &Vec<(String, String)>| {
            headers
                .iter()
                .find(|(key, _)| key == "X-Conversation-ID")
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(pick(&first), pick(&second), "同会话必须稳定");
        // 逐请求 id 必须不同（那是每个请求独有的）。
        let other = conversation_headers("wb-cn", "uid-1", Some("sess-2"));
        assert_ne!(pick(&first), pick(&other), "换会话要换 id");
    }

    /// 并入时要把旧的 `X-Request-ID` 换掉，不能同时发两个。
    #[test]
    fn merging_replaces_the_legacy_request_id() {
        let base = vec![
            ("X-Request-ID".to_string(), "legacy-000001".to_string()),
            ("Authorization".to_string(), "Bearer t".to_string()),
        ];
        let merged = with_conversation_identity(base, "wb-cn", "uid-1", Some("sess-1"));
        let request_ids: Vec<&String> = merged
            .iter()
            .filter(|(name, _)| name == "X-Request-ID")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(request_ids.len(), 1, "只能有一个 X-Request-ID");
        assert_ne!(request_ids[0], "legacy-000001", "旧值必须被替换");
        // 其它头要保留。
        assert!(merged
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Bearer t"));
    }

    #[test]
    fn fingerprints_are_stable_and_channel_scoped() {
        let hosts = hosts(Realm::Cn, Outbound::Desktop);
        let first = chat_headers("wb-cn", Realm::Cn, Outbound::Desktop, &hosts, &account(), 7);
        let second = chat_headers("wb-cn", Realm::Cn, Outbound::Desktop, &hosts, &account(), 8);
        let get = |headers: &Vec<(String, String)>, key: &str| {
            headers
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        assert_eq!(get(&first, "X-Machine-ID"), get(&second, "X-Machine-ID"));
        assert_ne!(get(&first, "X-Request-ID"), get(&second, "X-Request-ID"));

        let other_channel = chat_headers(
            "wb-intl",
            Realm::Cn,
            Outbound::Desktop,
            &hosts,
            &account(),
            7,
        );
        assert_ne!(
            get(&first, "X-Machine-ID"),
            get(&other_channel, "X-Machine-ID")
        );
    }

    #[test]
    fn billing_headers_declare_missing_enterprise_explicitly() {
        let headers = billing_headers(
            &hosts(Realm::Cn, Outbound::Desktop),
            &account(),
            Some("web"),
        );
        assert!(headers
            .iter()
            .any(|(key, value)| key == "X-No-Enterprise-Id" && value == "1"));
        assert!(headers
            .iter()
            .any(|(key, value)| key == "X-Client-Platform" && value == "web"));
    }

    #[test]
    fn enterprise_account_gets_tenant_headers_instead() {
        let mut account = account();
        account.enterprise_id = Some("ent-9".into());
        let headers = chat_headers(
            "wb-cn",
            Realm::Cn,
            Outbound::Desktop,
            &hosts(Realm::Cn, Outbound::Desktop),
            &account,
            1,
        );
        assert!(headers
            .iter()
            .any(|(key, value)| key == "X-Enterprise-Id" && value == "ent-9"));
        assert!(headers
            .iter()
            .any(|(key, value)| key == "X-Tenant-Id" && value == "ent-9"));
    }
}
