//! WorkBuddy 渠道适配器。国内版与国际版**共用这一份代码**，靠 `options` 区分：
//!
//! ```json
//! { "id": "wb-cn",   "adapter": "workbuddy", "options": {"realm": "cn",   "outbound_identity": "workbuddy"} }
//! { "id": "wb-intl", "adapter": "workbuddy", "options": {"realm": "intl", "outbound_identity": "workbuddy"} }
//! ```
//!
//! 同一个适配器实例化两次就是两个独立渠道 —— 这是「新增渠道核心零改动」的第一道验证。

pub mod auth;
pub mod billing;
pub mod daily;
mod growth;

/// 成长体系快照（加油站 / 连登 / 抽奖）。
///
/// 包一层是为了让 `server::admin` 不必知道 `GrowthSnapshot` 的类型：
/// 它只要一个结构体，字段名由 `json!` 那边决定。
pub async fn growth_snapshot(
    client: &reqwest::Client,
    account: &AccountSnapshot,
) -> growth::GrowthSnapshot {
    // 成长体系只有国内版有（国际版不适用），所以 realm 固定按国内版取域名。
    let hosts = identity::hosts(identity::Realm::Cn, identity::Outbound::Desktop);
    growth::snapshot(client, &hosts, account).await
}
pub mod tasks;
pub mod catalog;
pub mod live_models;
pub mod hygiene;
pub mod identity;

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;

use crate::gateway::channels::openai_compat;
use crate::gateway::{
    channels::{
        AccountSnapshot, ChannelAdapter, Credential, LoginPoll, LoginTicket, OpsCaps,
        UpstreamModel,
    },
    errors::{ErrorKind, ErrorScope, GatewayError},
    protocol::cir::{CanonicalEvent, CanonicalRequest},
    transport::UpstreamRequest,
};

use identity::{Hosts, Outbound, Realm};

pub struct WorkbuddyChannel {
    id: String,
    /// 渠道级兜底 realm（账号没填时用）。
    ///
    /// 为什么需要兜底：平台下沉到账号之后，一个 WorkBuddy 适配器实例要同时
    /// 服务"国内版"和"国际版"的账号。realm 变成**账号属性**，
    /// 但老数据（以及没填 realm 的账号）仍要有个默认值，否则会打到错的域名上。
    default_realm: Realm,
    default_outbound: Outbound,
    /// 单调序号：给 `X-Request-ID` 用，避免同账号高并发下撞 ID。
    sequence: AtomicU64,
}

impl WorkbuddyChannel {
    pub fn from_options(id: &str, options: &Value) -> Self {
        let realm = Realm::parse(
            options
                .get("realm")
                .and_then(Value::as_str)
                .unwrap_or("cn"),
        );
        let outbound = Outbound::parse(
            options
                .get("outbound_identity")
                .and_then(Value::as_str)
                .unwrap_or("workbuddy"),
        );
        Self {
            id: id.to_string(),
            default_realm: realm,
            default_outbound: outbound,
            sequence: AtomicU64::new(0),
        }
    }

    /// 解析这个**账号**用的 realm / 出站身份 / 域名。
    ///
    /// 顺序是账号优先、渠道兜底。平台下沉到账号之后，
    /// "这个号是哪个区的"必须跟着号走 —— 否则国内号与国际号共用一份
    /// hosts，其中一类必然打到错误域名（表现为 401 或 404）。
    fn identity_for(&self, account: &AccountSnapshot) -> (Realm, Outbound, Hosts) {
        let options = account.options.as_ref();
        let realm = options
            .and_then(|value| value.get("realm"))
            .and_then(Value::as_str)
            .map(Realm::parse)
            .unwrap_or(self.default_realm);
        let outbound = options
            .and_then(|value| value.get("outbound_identity"))
            .and_then(Value::as_str)
            .map(Outbound::parse)
            .unwrap_or(self.default_outbound);
        (realm, outbound, identity::hosts(realm, outbound))
    }

    fn snapshot_catalog(&self, realm: Realm) -> Vec<UpstreamModel> {
        match realm {
            Realm::Cn => catalog::snapshot_cn(),
            // 国际版目录与国内版差异较大，没有经过验证的快照就不编造。
            Realm::Intl => Vec::new(),
        }
    }

    pub async fn live_models(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<live_models::LiveModelCatalog, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        live_models::fetch(client, &hosts, realm, account).await
    }
}

#[async_trait]
impl ChannelAdapter for WorkbuddyChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_kind(&self) -> &str {
        "workbuddy"
    }

    /// WorkBuddy 的运营能力按 realm 区分（SPEC 11.1）：
    /// · 国内版有成长任务中心 + 签到 + 猫猫旅行；
    /// · 国际版**没有**任务清单，只有每日活跃对话。
    ///
    /// `checkin` 对两个 realm 都置 true：`sign_in` hook 内部按 realm 选端点。
    /// 国际版的签到接口是否真的存在**尚未实测**（SPEC 8.1④）；
    /// 若实测不存在，这里应改成只对 `Realm::Cn` 声明 true，
    /// 否则界面会给出一个点了必然失败的按钮。
    ///
    /// 这里返回的是**渠道级**能力（按兜底 realm 算）。账号级判断在
    /// `ops_caps_for`：同一个 WorkBuddy 适配器可能同时挂着国内号与国际号，
    /// 用渠道级能力去渲染国际号的按钮就会给出它做不了的动作。
    fn ops_caps(&self) -> OpsCaps {
        OpsCaps {
            tasks: self.default_realm == Realm::Cn,
            daily: true,
            checkin: true,
        }
    }

    fn ops_caps_for(&self, account: &AccountSnapshot) -> OpsCaps {
        let (realm, _, _) = self.identity_for(account);
        OpsCaps {
            tasks: realm == Realm::Cn,
            daily: true,
            checkin: true,
        }
    }

    fn build_chat_request(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<UpstreamRequest, GatewayError> {
        let body = hygiene::build_chat_body(&request.model, request);
        let (realm, outbound, hosts) = self.identity_for(account);
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let headers = identity::chat_headers(
            &self.id,
            realm,
            outbound,
            &hosts,
            account,
            sequence,
        );
        // **必须补上 X-Conversation-* 族**：缺了上游直接 11-128
        // （"Illegal API invocation from an unapproved channel"）。
        // 会话键取自调用方给的 session，保证多轮复用同一个 Conversation-ID。
        let headers = identity::with_conversation_identity(
            headers,
            &self.id,
            &account.uid,
            request.metadata.session_key().as_deref(),
        );
        Ok(UpstreamRequest {
            url: hosts.chat_url(catalog::CHAT_PATH),
            headers,
            body,
        })
    }

    fn parse_event(&self, frame: &str) -> Vec<CanonicalEvent> {
        openai_compat::parse_openai_chunk(frame)
    }

    fn classify_error(&self, status: u16, body: &str, upstream_model: &str) -> GatewayError {
        refine(status, body, upstream_model)
    }

    async fn list_models(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Vec<UpstreamModel>>, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        let snapshot = self.snapshot_catalog(realm);
        let Some((ids, meta)) = catalog::fetch_remote(client, &hosts, realm, account).await
        else {
            // 拉取失败（网络 / 非 2xx / 空列表）：用快照顶，绝不对外报"没有模型"。
            return Ok((!snapshot.is_empty()).then_some(snapshot));
        };
        let snapshot_credits = snapshot
            .iter()
            .map(|entry| (entry.id.clone(), entry.credits.clone()))
            .collect();
        let curated = catalog::curate(&ids, &meta, &snapshot_credits);
        Ok(Some(catalog::merge(&curated, &meta, &snapshot)))
    }

    async fn refresh(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Credential>, GatewayError> {
        let Some(refresh_token) = account.refresh_token.as_deref().filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let (_, _, hosts) = self.identity_for(account);
        auth::refresh(client, &hosts, account, refresh_token)
            .await
            .map(Some)
    }

    async fn login_start(
        &self,
        client: &reqwest::Client,
        platform: &str,
        realm: Option<&str>,
    ) -> Result<LoginTicket, GatewayError> {
        /*
          用**调用方指定的 realm**，而不是渠道兜底值。

          为什么必须这样：登录发生在建号之前，账号还不存在、没有 realm 可读。
          过去退回 `default_realm`（渠道级，通常是 cn），于是：
            · 用户在国际版分组里选"国际版"去授权；
            · 弹出的却是 `copilot.tencent.com` 的国内授权页；
            · 授权完拿到的凭据属于国内站，建出来的号跑不了国际模型。

          realm 缺省时才用兜底值 —— 那是老调用方的兼容路径。
        */
        let resolved = realm.map(identity::Realm::parse).unwrap_or(self.default_realm);
        let hosts = identity::hosts(resolved, self.default_outbound);
        auth::login_start(client, &hosts, platform).await
    }

    async fn login_poll(
        &self,
        client: &reqwest::Client,
        state: &str,
    ) -> Result<LoginPoll, GatewayError> {
        let hosts = identity::hosts(self.default_realm, self.default_outbound);
        auth::login_poll(client, &hosts, state).await
    }

    async fn list_tasks(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Vec<crate::gateway::channels::TaskInfo>>, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        if realm != Realm::Cn {
            // 国际版没有国内成长任务中心。
            return Ok(None);
        }
        tasks::list(client, &hosts, account).await.map(Some)
    }

    async fn run_tasks(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        only: &[String],
        progress: tokio::sync::mpsc::UnboundedSender<crate::gateway::channels::TaskRunResult>,
    ) -> Result<Vec<crate::gateway::channels::TaskRunResult>, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        if realm != Realm::Cn {
            return Ok(Vec::new());
        }
        tasks::run(client, &hosts, account, only, progress).await
    }

    async fn claim_task(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        code: &str,
    ) -> Result<crate::gateway::channels::TaskRunResult, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        if realm != Realm::Cn {
            return Err(GatewayError::unsupported("国际版没有成长任务体系"));
        }
        // 领奖的失败原因（未达标/已领过）是**行内文案**，不是异常：返回 Ok 交给面板展示。
        Ok(tasks::claim(client, &hosts, account, code).await)
    }

    async fn fetch_quota(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<crate::gateway::channels::QuotaSnapshot>, GatewayError> {
        let (_, _, hosts) = self.identity_for(account);
        billing::fetch(client, &hosts, account).await
    }

    async fn daily_actions(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Vec<crate::gateway::channels::DailyAction>>, GatewayError> {
        // 动作按 realm 分开（CN：签到 + 猫猫旅行；Intl：每日活跃对话）。
        let (realm, _, hosts) = self.identity_for(account);
        Ok(Some(daily::actions(client, &hosts, realm, account).await))
    }

    async fn run_daily_action(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        code: &str,
    ) -> Result<crate::gateway::channels::TaskRunResult, GatewayError> {
        let (realm, _, hosts) = self.identity_for(account);
        Ok(daily::run(client, &hosts, realm, account, code).await)
    }

    async fn sign_in(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<bool, GatewayError> {
        let (_, _, hosts) = self.identity_for(account);
        Ok(auth::sign_in(client, &hosts, account).await)
    }
}

/// 上游错误语义：先在正文里找已知数字码，找不到就按状态码兜底。
///
/// 数字码比状态码精确得多 —— 例如 12403 是「UA 不被接受」，按状态码只会看到 400。
///
/// `upstream_model` 只用来描述**范围**：这个渠道的限额是按模型的，
/// 6004 要落到 [`ErrorScope::AccountModel`]，不能拉黑整个账号。
pub fn refine(status: u16, body: &str, upstream_model: &str) -> GatewayError {
    // 只认结构化错误码，避免请求 ID 或正文中碰巧出现的数字触发账号限制。
    if serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("code").cloned())
        .is_some_and(|code| code.as_i64() == Some(11140) || code.as_str() == Some("11140"))
    {
        return GatewayError::new(
            ErrorKind::AccountRestricted,
            ErrorScope::Account,
            403,
            "账号存在异常行为，已被上游限制使用（错误码 11140）；请通过官方反馈渠道申诉",
            false,
        )
        .with_upstream(openai_compat::raw(body));
    }
    // 渠道校验单独分类，允许有限重试但不惩罚账号。
    // 客户端文本和出站身份不兼容都可能稳定触发，不能承诺换号即可恢复。
    let lowered = body.to_ascii_lowercase();
    if lowered.contains("11128") || lowered.contains("unapproved channel") {
        return GatewayError::new(
            ErrorKind::ChannelBlocked,
            ErrorScope::Request,
            429,
            "上游渠道校验拦截：请检查客户端请求的兼容性及出站身份配置",
            true,
        )
        .with_upstream(openai_compat::raw(body));
    }
    let refined = refine_kind(status, body, upstream_model);
    refined.with_upstream(openai_compat::raw(body))
}

fn refine_kind(status: u16, body: &str, upstream_model: &str) -> GatewayError {
    match known_code(body) {
        Some(12403) => GatewayError::invalid_request(
            "上游拒绝了当前客户端标识（UA）：检查渠道的出站身份配置",
        ),
        Some(11135) => {
            GatewayError::invalid_request("图片数据非法：换一张真图或新开会话再试")
        }
        Some(11133) => GatewayError::invalid_request(openai_compat::short(body, "请求参数不合法")),
        Some(11148) => {
            GatewayError::invalid_request("工具调用序列断裂（本地自愈未覆盖），建议重开会话")
        }
        // 6004 = **模型级**限额（参考实现的原话："usage exceeds frequency limit"，
        // 正文会给 "your usage will reset at <时间>"）。必须落到该模型：
        // 同账号的其他模型照常能跑 —— 早先写成 Account，等于一个模型限额把整个账号拉黑。
        // 拿不到 reset 时刻也算模型级（池子会给它一个模型退避），不能因此停整个账号。
        Some(6004) => {
            let reset_at = openai_compat::parse_rate_limit_reset(body);
            // 恢复时刻写进提示里：客户端/面板要能用它决定"等还是换模型"。
            let when = match reset_at {
                Some(until) => {
                    let remaining = until.saturating_sub(crate::gateway::timeutil::now_ms());
                    format!(
                        "，预计 {} 后恢复",
                        crate::gateway::core::pool::humanize(remaining)
                    )
                }
                // 上游没给时刻：如实说"我们自己按模型退避"，别让人以为是上游承诺。
                None => "，恢复时刻未知（上游未给出，按模型退避重试）".to_string(),
            };
            GatewayError::new(
                ErrorKind::RateLimited,
                ErrorScope::AccountModel(upstream_model.to_string()),
                429,
                format!("该模型被上游限流{when}（同账号其他模型不受影响）"),
                true,
            )
            .with_reset_at(reset_at)
        }
        _ => openai_compat::classify_openai_error(status, body),
    }
}

/// 按独立数字 token 匹配错误码，避免把正文里的 token 计数当成错误码。
fn known_code(body: &str) -> Option<i64> {
    const CODES: [i64; 6] = [12403, 11135, 11133, 11148, 6004, 11-128];
    CODES
        .into_iter()
        .find(|code| contains_code(body, *code))
}

fn contains_code(body: &str, code: i64) -> bool {
    let needle = code.to_string();
    let bytes = body.as_bytes();
    let mut start = 0;
    while let Some(index) = body[start..].find(&needle) {
        let position = start + index;
        let before_ok = position == 0 || !bytes[position - 1].is_ascii_digit();
        let after = position + needle.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = position + needle.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 账号没有自己的 realm 时，用渠道兜底值。
    #[test]
    fn realm_and_identity_fall_back_to_channel_defaults() {
        let cn = WorkbuddyChannel::from_options("wb-cn", &json!({"realm": "cn"}));
        let (realm, outbound, hosts) = cn.identity_for(&AccountSnapshot::default());
        assert_eq!(realm, Realm::Cn);
        assert_eq!(outbound, Outbound::Desktop);
        assert_eq!(hosts.chat, "copilot.tencent.com");

        let intl = WorkbuddyChannel::from_options(
            "wb-intl",
            &json!({"realm": "intl", "outbound_identity": "vscode"}),
        );
        let (realm, outbound, hosts) = intl.identity_for(&AccountSnapshot::default());
        assert_eq!(realm, Realm::Intl);
        assert_eq!(outbound, Outbound::Vscode);
        assert_eq!(hosts.chat, "www.workbuddy.cn");
    }

    /// **账号的 realm 优先于渠道**：这是"平台下沉到账号"之后的关键保证。
    ///
    /// 反过来的话，同一个 WorkBuddy 适配器服务国内号与国际号时，
    /// 其中一类必然打到错误域名 —— 表现是 401/404，但看配置完全正常。
    #[test]
    fn account_realm_overrides_the_channel_default() {
        let channel = WorkbuddyChannel::from_options("workbuddy", &json!({"realm": "cn"}));
        let intl_account = AccountSnapshot {
            options: Some(json!({"realm": "intl", "outbound_identity": "vscode"})),
            ..Default::default()
        };
        let (realm, outbound, hosts) = channel.identity_for(&intl_account);
        assert_eq!(realm, Realm::Intl, "账号写的是国际版");
        assert_eq!(outbound, Outbound::Vscode);
        assert_eq!(hosts.chat, "www.workbuddy.cn");

        // 能力也要跟着账号走：国际号不该声明有成长任务。
        let caps = channel.ops_caps_for(&intl_account);
        assert!(!caps.tasks, "国际版没有成长任务中心");
        assert!(caps.daily);
    }

    #[test]
    fn numeric_codes_are_translated_more_precisely_than_status_codes() {
        let ua = refine(400, r#"{"code":12403,"msg":"unknown user agent"}"#, "m");
        assert!(ua.message.contains("客户端标识"), "{}", ua.message);
        assert!(!ua.retryable, "配置问题重试没用");

        let tools = refine(400, "code 11148 tool call sequence broken", "m");
        assert!(tools.message.contains("工具调用序列断裂"), "{}", tools.message);

        let image = refine(400, "{\"code\":11135}", "m");
        assert!(image.message.contains("图片数据非法"), "{}", image.message);

        // 6004 是模型级限流，带 reset 时刻 —— 范围必须落到那个模型。
        let limited = refine(
            429,
            "code 6004 busy, your usage will reset at 1970-01-02 00:00:00 UTC+0",
            "deepseek-v4.1-flash",
        );
        assert_eq!(limited.kind, ErrorKind::RateLimited);
        assert_eq!(limited.reset_at_ms, Some(86_400_000));
        assert_eq!(
            limited.scope,
            ErrorScope::AccountModel("deepseek-v4.1-flash".into()),
            "6004 只停这一个模型，不是整个账号"
        );

        // 参考实现里 6004 的真实措辞：恢复时刻用 "your usage will reset at ... UTC+8"。
        let real = refine(
            429,
            r#"{"code":6004,"msg":"your usage exceeds frequency limit, your usage will reset at 2026-09-19 18:29:03 UTC+8"}"#,
            "deepseek-v4.1-flash",
        );
        assert_eq!(
            real.reset_at_ms,
            Some(1_789_813_743_000),
            "要能读懂上游给的恢复时刻（北京时间）"
        );

        // 真机实测报文（中文！我们发 Accept-Language: zh-CN，上游就把文案本地化了）：
        // 认死英文 "reset at" 会解析不出恢复时刻，退化成 10m/20m 退避 —— 而真实恢复在第二天。
        let real_cn = refine(
            429,
            r#"{"code":6004,"msg":"您的使用量已超出频率限制，将在 2026-10-11 14:46:40 UTC+8 重置，您也可以切换其他模型继续使用。","requestId":"a9d6fe19_000001"}"#,
            "deepseek-v4.1-flash",
        );
        assert_eq!(
            real_cn.reset_at_ms,
            Some(1_791_701_200_000),
            "中文文案里的恢复时刻也要能解析出来（2026-10-11 14:46:40 +08:00）"
        );
        assert!(
            real_cn.message.contains("后恢复"),
            "提示要带上剩余时间：{}",
            real_cn.message
        );
        assert!(!real_cn.message.contains("未知"), "{}", real_cn.message);

        // 没有 reset 时刻也要保持模型级（池子给模型退避，账号照用）。
        let bare = refine(429, r#"{"code":6004,"msg":"usage exceeds frequency limit"}"#, "glm-5.3");
        assert_eq!(bare.reset_at_ms, None);
        assert_eq!(bare.scope, ErrorScope::AccountModel("glm-5.3".into()));

        // 裸 429（没有 6004）= 账号级软限流，仍按账号退避（与参考实现一致）。
        let soft = refine(429, "too many requests", "glm-5.3");
        assert_eq!(soft.scope, ErrorScope::Account);
    }

    /// 分类后的错误必须**留着上游原文**：面板/请求日志/探针要能看到上游到底说了什么。
    /// （只留一句人话提示，等于出事时把线索丢掉。）
    #[test]
    fn classified_errors_keep_the_upstream_body() {
        let body = r#"{"code":6004,"msg":"usage exceeds frequency limit, your usage will reset at 1970-01-02 00:00:00 UTC+0"}"#;
        let error = refine(429, body, "deepseek-v4.1-flash");
        assert!(
            error.upstream.as_deref().is_some_and(|raw| raw.contains("6004")),
            "原文要留着"
        );
        let described = error.describe();
        assert!(described.contains("上游返回"), "{described}");
        assert!(described.contains("reset at"), "{described}");
        // 能算出恢复时刻时，提示里要写清楚（客户端据此决定等还是换模型）。
        assert!(error.message.contains("后恢复"), "{}", error.message);

        // 通用分类路径同样保留原文。
        let generic = refine(429, "slow down please", "glm-5.3");
        assert_eq!(generic.upstream.as_deref(), Some("slow down please"));
    }

    #[test]
    fn account_restriction_is_not_content_policy() {
        let body = r#"{"code":11140,"msg":"request illegal","requestId":"restriction-request","displayMsg":{"zh":"账号存在异常行为，已被限制使用"},"displayTips":{"zh":"请提交反馈申诉"}}"#;
        for status in [400, 403] {
            let error = refine(status, body, "glm-5.3");
            assert_eq!(error.kind, ErrorKind::AccountRestricted);
            assert_eq!(error.scope, ErrorScope::Account);
            assert!(!error.retryable);
            assert_eq!(error.status, 403);
            assert!(error.message.contains("限制使用"));
            assert!(error.describe().contains("restriction-request"));
            assert!(error.describe().contains("请提交反馈申诉"));
        }
        assert_eq!(refine(403, r#"{"code":"11140"}"#, "m").kind, ErrorKind::AccountRestricted);
        assert_eq!(refine(403, r#"{"code":999,"requestId":"11140"}"#, "m").kind, ErrorKind::ContentPolicy);
    }

    /// 渠道校验保留请求级错误、重试标记和上游原文。
    #[test]
    fn channel_block_11_128_is_retryable_request_scope() {
        let body = r#"{"code":11128,"msg":"Illegal API invocation from an unapproved channel","displayTips":{"zh":"请求被拦截，请重新发送"}}"#;
        let error = refine(400, body, "glm-5.3");
        assert_eq!(error.kind, ErrorKind::ChannelBlocked);
        assert_eq!(error.scope, ErrorScope::Request);
        assert!(error.retryable);
        assert!(!error.message.contains("随机"));
        assert!(error.upstream.as_deref().is_some_and(|raw| raw.contains("unapproved")));
        // 英文措辞也要认。
        let plain = refine(400, "Illegal API invocation from an unapproved channel", "glm-5.3");
        assert_eq!(plain.kind, ErrorKind::ChannelBlocked);
    }

    #[test]
    fn unknown_body_falls_back_to_status_semantics() {
        let error = refine(401, "unauthorized", "m");
        assert_eq!(error.kind, ErrorKind::AuthExpired);
        assert!(error.retryable);
    }

    #[test]
    fn code_matching_requires_token_boundaries() {
        // 正文里的 token 计数 124030 不该被当成 12403。
        assert!(known_code("code 12403").is_some());
        assert!(known_code("124030").is_none());
        assert!(known_code("tokens: 111335").is_none());
    }
}
