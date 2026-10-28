//! 自定义供应商渠道：把任意 OpenAI / Anthropic 兼容端点接进号池。
//!
//! 与 workbuddy 这种「协议 + 私有错误码」的渠道不同，这里没有上游特有的身份伪装、
//! WAF 清洗或计费逻辑 —— 它只做三件事：
//! ① 按配置把 CIR 请求编成上游报文（OpenAI Chat Completions 或 Anthropic Messages）；
//! ② 把上游 SSE 帧翻回 CIR 事件；
//! ③ 把模型清单报给矩阵（模型与元数据由用户在面板里填，或由「获取模型」拉取）。
//!
//! 为什么要有它：号池不该只认识内置渠道。中转站、自建反代、本地推理服务
//! （Ollama / LM Studio / vLLM）都讲 OpenAI 兼容协议，接进来即可被调度、
//! 被限流、被用量统计，和内置渠道享受同一套核心能力，核心一行不用改。
//!
//! **刻意不做指纹清洗**：`workbuddy::hygiene::sanitize_text` 是为绕过某个特定上游的
//! WAF 而写的（它会把 "Claude Code" 之类的串改写成等价写法）。对通用供应商套用
//! 会改写用户正文 —— 一个正当的供应商不该收到被篡改的内容。
//!
//! 分文件按职责：`catalog`（模型清单 + 通用「获取模型」）、`encode`（CIR → 上游报文）、
//! `decode`（上游 SSE → CIR）。

mod catalog;
mod decode;
mod encode;

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;

use crate::gateway::{
    channels::{openai_compat, AccountSnapshot, ChannelAdapter, Credential, UpstreamModel},
    errors::GatewayError,
    protocol::cir::{CanonicalEvent, CanonicalRequest},
    transport::UpstreamRequest,
};

use catalog::configured_models;
use decode::{parse_anthropic_event, parse_responses_event};
use encode::{build_anthropic_body, build_openai_body, build_responses_body};

#[cfg(test)]
mod tests;

/// Anthropic 只认这个版本号；缺了直接 400。
pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 鉴权头：按协议选 Bearer 还是 x-api-key。**两类请求共用的唯一实现**。
///
/// Anthropic 只用 `x-api-key`（发 `Authorization: Bearer` 会 401）；
/// OpenAI 兼容面两个都发 —— 中转站有的只认其中一种，同时带上对两者都无害。
pub(super) fn protocol_auth_headers(
    protocol: UpstreamProtocol,
    account: &AccountSnapshot,
) -> Vec<(String, String)> {
    let key = account.access_token.clone().unwrap_or_default();
    if key.is_empty() {
        return Vec::new();
    }
    match protocol {
        UpstreamProtocol::Anthropic => vec![
            ("x-api-key".into(), key),
            ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
        ],
        // Bearer + x-api-key 都发：中转站有的只认其中一种，同时带上对两者都无害。
        UpstreamProtocol::OpenAi | UpstreamProtocol::Responses => vec![
            ("Authorization".into(), format!("Bearer {key}")),
            ("x-api-key".into(), key),
        ],
    }
}

/// 拼接地址，容忍任一侧的斜杠。
pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// 上游协议。决定报文形状、SSE 形状与鉴权头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamProtocol {
    /// OpenAI Chat Completions（`/chat/completions`）。绝大多数中转站与本地服务。
    OpenAi,
    /// OpenAI Responses（`/responses`）。新版 OpenAI 与兼容它的中转。
    Responses,
    /// Anthropic Messages（`/messages`）。Claude 官方与兼容反代。
    Anthropic,
}

impl UpstreamProtocol {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "anthropic-messages" | "messages" | "claude" => Self::Anthropic,
            "responses" | "openai-responses" => Self::Responses,
            _ => Self::OpenAi,
        }
    }
}

pub struct CustomChannel {
    id: String,
    /// 渠道级兜底地址（现在通常为空）。
    ///
    /// 真正的地址在**账号**上：同一份计价策略下面可以挂多个不同上游的 Key。
    /// 渠道级这一份只在账号没填时兜底，保留它是为了兼容早期配置。
    base_url: String,
    /// 渠道级兜底协议（同上，通常为空，以账号为准）。
    protocol: UpstreamProtocol,
    /// 配置里的模型清单：矩阵的候选就来自它，**它就是事实来源**。
    ///
    /// 刻意不在 `list_models` 里回上游重拉：用户在面板里勾选过要发布哪些模型，
    /// 重拉会把取消勾选的模型又带回来。
    models: Vec<UpstreamModel>,
    /// 是否需要密钥。本地服务（Ollama 等）可以没有。
    require_api_key: bool,
    /// 单调序号：给 X-Request-ID 用，避免同账号高并发下撞 ID。
    sequence: AtomicU64,
    /// 最近一次出站报文的**结构摘要**（不含正文）。
    ///
    /// 上游报"参数非法"时，只回一句 `The request param is invalid`，
    /// 不说哪个字段。把形状记下来，错误里就能直接指出"我们发了哪些键"，
    /// 不用再靠猜或翻日志。刻意只存结构：正文是用户数据。
    last_shape: std::sync::Mutex<Option<String>>,
}

impl CustomChannel {
    pub fn from_options(id: &str, options: &Value) -> Self {
        let base_url = options
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        Self {
            id: id.to_string(),
            base_url,
            protocol: UpstreamProtocol::parse(
                options
                    .get("protocol")
                    .and_then(Value::as_str)
                    .unwrap_or("openai-chat"),
            ),
            models: configured_models(options),
            require_api_key: options
                .get("require_api_key")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            sequence: AtomicU64::new(0),
            last_shape: std::sync::Mutex::new(None),
        }
    }

    /// 该账号实际使用的基址：**账号优先，渠道兜底**。
    ///
    /// 顺序不能反：账号上填的是"这把 Key 属于哪家上游"，
    /// 渠道上那份是历史遗留的默认值。反过来的话，
    /// 用户改账号地址会毫无效果（被渠道值盖住），又一个"改了没用"。
    fn base_url_for(&self, account: &AccountSnapshot) -> String {
        account
            .base_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.base_url)
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    /// 该账号实际使用的协议：同样账号优先。
    fn protocol_for(&self, account: &AccountSnapshot) -> UpstreamProtocol {
        match account.protocol.as_deref() {
            Some(value) if !value.trim().is_empty() => UpstreamProtocol::parse(value),
            _ => self.protocol,
        }
    }

    /// 对话端点。地址已是完整端点时不再拼接。
    fn chat_url(&self, base_url: &str, protocol: UpstreamProtocol) -> String {
        let base = base_url.trim_end_matches('/');
        // 用户粘的就是完整端点：不管配置里选了哪个协议，都别再拼一次
        // （否则 "/v1/responses" 会被拼成 "/v1/responses/chat/completions"）。
        if base.ends_with("/chat/completions")
            || base.ends_with("/responses")
            || base.ends_with("/messages")
        {
            return base.to_string();
        }
        match protocol {
            UpstreamProtocol::OpenAi => join_url(base, "chat/completions"),
            UpstreamProtocol::Responses => join_url(base, "responses"),
            UpstreamProtocol::Anthropic => join_url(base, "messages"),
        }
    }

    fn auth_headers(&self, account: &AccountSnapshot) -> Vec<(String, String)> {
        let mut headers = vec![("Content-Type".into(), "application/json".into())];
        headers.extend(protocol_auth_headers(self.protocol_for(account), account));
        headers
    }

    fn model_meta(&self, id: &str) -> Option<&UpstreamModel> {
        self.models.iter().find(|model| model.id == id)
    }

    /// 识别藏在 200 SSE 流里的业务错误；命中时把**出站报文结构**一并带进消息。
    ///
    /// 上游只说 `The request param is invalid`，不说是哪个字段 —— 而这条链路
    /// 的报文是从客户端的原始 body 派生来的（Responses / Anthropic 专有字段
    /// 会被一并带出）。把顶层键与关键取值列出来，才能一眼看出问题。
    fn embedded_error(&self, frame: &str) -> Option<String> {
        let value: Value = serde_json::from_str(frame).ok()?;
        let code = value
            .get("error_code")
            .or_else(|| value.get("errorCode"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|code| !code.is_empty() && *code != "0")?;
        let message = value
            .get("error_msg")
            .or_else(|| value.get("errorMsg"))
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let shape = self
            .last_shape
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_else(|| "（无记录）".into());
        Some(format!("{code} {message}｜出站报文结构：{shape}"))
    }
}

/// JSON 值的类型名（`Value::type_name` 在本版本不可用）。
fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 出站报文的**结构摘要**：顶层键 + 几个容易踩雷的取值，不含任何正文。
///
/// 用来回答"上游说参数非法，到底哪一项"。`tools`/`messages` 只报数量与角色，
/// 不把用户内容写进错误消息或日志。
fn body_shape(body: &Value) -> String {
    let Some(map) = body.as_object() else {
        return format!("（非对象：{}）", value_kind(body));
    };
    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();

    let mut parts = vec![format!(
        "keys=[{}]",
        keys.join(",")
    )];
    if let Some(messages) = map.get("messages").and_then(Value::as_array) {
        let roles: Vec<&str> = messages
            .iter()
            .filter_map(|message| message.get("role").and_then(Value::as_str))
            .collect();
        let tool_calls = messages
            .iter()
            .filter(|message| message.get("tool_calls").is_some())
            .count();
        parts.push(format!("messages={} 角色=[{}]", messages.len(), roles.join(",")));
        if tool_calls > 0 {
            parts.push(format!("带 tool_calls 的消息={tool_calls}"));
        }
    }
    if let Some(tools) = map.get("tools").and_then(Value::as_array) {
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .or_else(|| tool.get("name"))
                    .and_then(Value::as_str)
            })
            .collect();
        parts.push(format!("tools={} [{}]", tools.len(), names.join(",")));
    }
    if let Some(choice) = map.get("tool_choice") {
        let shown = match choice {
            Value::String(text) => format!("\"{text}\""),
            other => format!("对象({})", value_kind(other)),
        };
        parts.push(format!("tool_choice={shown}"));
    }
    for key in ["max_tokens", "max_output_tokens", "reasoning_effort", "temperature", "n"] {
        if let Some(value) = map.get(key) {
            parts.push(format!("{key}={value}"));
        }
    }
    // 这些是**别的协议**专有的键，出现在 chat 报文里通常就是"参数非法"的来源。
    let foreign: Vec<&str> = [
        "reasoning",
        "text",
        "include",
        "store",
        "truncation",
        "prompt_cache_key",
        "parallel_tool_calls",
        "previous_response_id",
        "thinking",
        "top_k",
        "stop_sequences",
    ]
    .into_iter()
    .filter(|key| map.contains_key(*key))
    .collect();
    if !foreign.is_empty() {
        parts.push(format!("⚠ 跨协议字段=[{}]", foreign.join(",")));
    }
    parts.join(" ")
}

#[async_trait]
impl ChannelAdapter for CustomChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_kind(&self) -> &str {
        "custom"
    }

    /// 密钥是用户填的静态值，没有刷新流程 —— 保鲜链路据此跳过刷新。
    fn static_credential(&self) -> bool {
        true
    }

    fn requires_credential(&self) -> bool {
        self.require_api_key
    }

    fn build_chat_request(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<UpstreamRequest, GatewayError> {
        if self.base_url.is_empty() && account.base_url.as_deref().unwrap_or("").is_empty() {
            return Err(GatewayError::internal(format!(
                "账号 {}/{} 没有配置上游地址",
                self.id, account.uid
            )));
        }
        let base_url = self.base_url_for(account);
        let protocol = self.protocol_for(account);
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut headers = self.auth_headers(account);
        headers.push((
            "X-Request-ID".into(),
            crate::gateway::security::request_id(&self.id, &account.uid, sequence),
        ));

        let body = match protocol {
            UpstreamProtocol::OpenAi => build_openai_body(request),
            UpstreamProtocol::Responses => build_responses_body(request),
            UpstreamProtocol::Anthropic => {
                let max_output = self.model_meta(&request.model).and_then(|m| m.max_output_tokens);
                build_anthropic_body(request, max_output)
            }
        };
        // 记下出站报文的**结构**（不含正文），上游报"参数非法"时能直接指出是谁的问题。
        if let Ok(mut slot) = self.last_shape.lock() {
            *slot = Some(body_shape(&body));
        }
        Ok(UpstreamRequest {
            url: self.chat_url(&base_url, protocol),
            headers,
            body,
        })
    }

    fn parse_event(&self, frame: &str) -> Vec<CanonicalEvent> {
        // 上游会把业务错误塞进 HTTP 200 的流里（华为系 CodeArts / ModelArts 都这样）：
        // `{"text":"[DONE]","error_code":"InferHub.001001005.400",
        //   "error_msg":"The request param is invalid"}`。
        // 不识别的话这一帧没有 choices，会被当成"空内容"静默吞掉，
        // 最后记成「完成 / 输出 0」——真正的失败就丢了。
        if let Some(error) = self.embedded_error(frame) {
            return vec![CanonicalEvent::Error { message: error }];
        }
        match self.protocol {
            UpstreamProtocol::OpenAi => openai_compat::parse_openai_chunk(frame),
            UpstreamProtocol::Responses => parse_responses_event(frame),
            UpstreamProtocol::Anthropic => parse_anthropic_event(frame),
        }
    }

    fn classify_error(&self, status: u16, body: &str, _upstream_model: &str) -> GatewayError {
        // 状态码语义是跨协议共通的（401 凭据 / 402 额度 / 429 限流 / 5xx 上游异常），
        // 这一份映射对两类供应商都成立。
        openai_compat::classify_openai_error(status, body)
    }

    /// 目录就是配置里的清单：自定义供应商没有标准元数据端点可拉。
    async fn list_models(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Vec<UpstreamModel>>, GatewayError> {
        Ok(Some(self.models.clone()))
    }

    /// 静态密钥没有刷新概念；返回 None 让上层保持原凭据。
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Credential>, GatewayError> {
        Ok(None)
    }

    /// 自定义供应商不提供登录流：密钥由用户在面板里填。
    /// （`login_start` 的默认实现已经返回可读的 unsupported，这里不再覆盖。）
    async fn sign_in(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<bool, GatewayError> {
        Ok(false)
    }
}
