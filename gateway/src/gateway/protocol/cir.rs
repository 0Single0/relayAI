//! Canonical 中间表示：请求、消息块、工具、事件、用量。

use serde::{Deserialize, Serialize};

/// `messages` 里的角色。system/developer 在解码时被提到 [`CanonicalRequest::system`]，
/// 因此这里没有 System：上游也只认 system/user/assistant/tool，归一后的消息流不再混入 system。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text { text: String },
    Image { url: String },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// 取文本内容；图片块返回 None（会话键的内容指纹会跳过它）。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    /// 上游把 arguments 当字符串分片传；这里始终是字符串。
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: Vec<Block>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 历史 assistant 消息里的思考内容。deepseek 系上游要求它在开启思考时非空
    /// （缺失会报错），所以既要保留客户端的原值，也要能在缺省时回填空串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reasoning {
    pub enabled: Option<bool>,
    /// low / medium / high；none 表示显式关闭思考。
    pub effort: Option<String>,
}

/// 会话键的三个来源。推导优先级见 `session_key()`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// `metadata.user_id` 原文（某些客户端把 session 塞在这里）。
    pub user_id: Option<String>,
    /// 请求体里的会话线索：`conversation_id` / `session_id` / `prompt_cache_key`。
    pub session_id: Option<String>,
    /// 请求头里的会话线索：`x-conversation-id` / `session-id` / `x-session-id`。
    pub header_session: Option<String>,
}

impl RequestMetadata {
    /// 会话键：头 > 体 > user_id。
    ///
    /// 只覆盖"客户端显式给了锚点"的两级；内容前缀指纹（客户端什么都没给时）由 P1 的亲和模块补。
    /// 空串一律当作"没给"，继续往下一级找 —— 否则一个空头就会顶掉请求体里的会话键。
    pub fn session_key(&self) -> Option<String> {
        [
            &self.header_session,
            &self.session_id,
            &self.user_id,
        ]
        .into_iter()
        .flatten()
        .find(|key| !key.is_empty())
        .cloned()
    }
}

#[derive(Debug, Clone)]
pub struct CanonicalRequest {
    pub model: String,
    /// 归一后的 system/developer 块（保序）。
    pub system: Vec<Block>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// 透传：归一化留给渠道适配器（`required` → `any` 之类）。
    pub tool_choice: Option<serde_json::Value>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// `max_tokens` 与 `max_completion_tokens` 归一后的值。
    pub max_tokens: Option<u64>,
    pub stream: bool,
    pub reasoning: Option<Reasoning>,
    pub metadata: RequestMetadata,
    /// 原始请求体：渠道特有的透传字段（如 `thinking`、`stream_options`）从这里取。
    pub raw: serde_json::Value,
}

impl CanonicalRequest {
    /// 决策追踪用的一行摘要（不含正文，只有形状与哈希级别的信息）。
    pub fn trace_summary(&self) -> String {
        let effort = self
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.effort.clone())
            .unwrap_or_else(|| "-".into());
        let tool_choice = match &self.tool_choice {
            None => "-",
            Some(serde_json::Value::String(kind)) => kind.as_str(),
            Some(_) => "custom",
        };
        format!(
            "model={} stream={} system={} messages={} tools={} tool_choice={} max_tokens={:?} temp={:?} top_p={:?} effort={} session={}",
            self.model,
            self.stream,
            self.system.len(),
            self.messages.len(),
            self.tools.len(),
            tool_choice,
            self.max_tokens,
            self.temperature,
            self.top_p,
            effort,
            self.metadata.session_key().unwrap_or_else(|| "-".into()),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub reasoning_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    /// 上游积分口径，与 token 并列返回。
    pub credit: Option<f64>,
}

/// 上游流式输出的统一事件。渠道适配器把上游 SSE 帧翻译成它，编码器再翻成下游协议。
#[derive(Debug, Clone)]
pub enum CanonicalEvent {
    Text { text: String },
    Reasoning { text: String },
    ToolCall {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    Usage(Usage),
    Finish { reason: String },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_prefers_header_then_body_then_user() {
        let mut metadata = RequestMetadata::default();
        assert_eq!(metadata.session_key(), None);
        metadata.user_id = Some("user-1".into());
        assert_eq!(metadata.session_key().as_deref(), Some("user-1"));
        metadata.session_id = Some("conv-2".into());
        assert_eq!(metadata.session_key().as_deref(), Some("conv-2"));
        metadata.header_session = Some("hdr-3".into());
        assert_eq!(metadata.session_key().as_deref(), Some("hdr-3"));
        // 空串视作没给。
        metadata.header_session = Some(String::new());
        assert_eq!(metadata.session_key().as_deref(), Some("conv-2"));
    }
}
