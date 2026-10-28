//! 测试与自检用的假渠道：不碰网络，就地吐 CIR 事件。
//!
//! 两个用途：① 没有真实账号时也能端到端跑通调度链路（重试/冷却/并发槽）；
//! ② 单测里可编程注入 401/429/5xx，验证换号与退避策略。

use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use futures_util::stream;
use serde_json::{json, Value};

use crate::gateway::{
    channels::{openai_compat, AccountSnapshot, ChannelAdapter, UpstreamModel},
    errors::GatewayError,
    protocol::cir::{Block, CanonicalEvent, CanonicalRequest, Usage},
    transport::{FrameStream, UpstreamRequest},
};

pub struct FakeChannel {
    id: String,
    model: String,
    /// 前 N 次请求直接失败，用来验证换号与退避。
    remaining_failures: AtomicU32,
    fail_status: u16,
}

impl FakeChannel {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            model: "fake-model".to_string(),
            remaining_failures: AtomicU32::new(0),
            fail_status: 429,
        }
    }

    /// 从渠道配置的 `options` 建实例：`{"model":"fake-model","fail_first":1,"fail_status":429}`。
    pub fn from_options(id: &str, options: &Value) -> Self {
        let mut channel = Self::new(id);
        channel.model = options
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("fake-model")
            .to_string();
        channel.remaining_failures = AtomicU32::new(
            options
                .get("fail_first")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
        );
        channel.fail_status = options
            .get("fail_status")
            .and_then(Value::as_u64)
            .unwrap_or(429) as u16;
        channel
    }

    /// 帧内容：思考 + 两段文本（回显最后一条用户消息）+ 用量 + 结束。
    ///
    /// **带 tools 时改为发一轮工具调用**：先吐 `tool_call`（分两片传 arguments，
    /// 模拟上游真实的增量形状），再以 `tool_calls` 结束。
    /// 这样"三种协议的工具调用闭环"才有东西可测 ——
    /// 只回文本的假渠道永远验不到 tool_calls 的编码路径。
    fn frames(&self, request: &CanonicalRequest) -> Vec<String> {
        let echo = request
            .messages
            .iter()
            .rev()
            .find_map(|message| message.content.iter().find_map(Block::as_text))
            .unwrap_or("")
            .to_string();

        // 客户端给了工具，就回一轮工具调用。用第一个工具的真实名字，
        // 这样编码器要处理的 name/arguments 与真实场景一致。
        if let Some(tool) = request.tools.first() {
            let id = "call_fake_1";
            return vec![
                json!({"kind": "reasoning", "text": "（假渠道思考：该调工具了）"}).to_string(),
                // arguments 分两片：验证增量拼接，而不是一次给全。
                json!({"kind": "tool_call", "index": 0, "id": id, "name": tool.name,
                       "arguments": "{\"city\":"})
                .to_string(),
                json!({"kind": "tool_call", "index": 0, "id": id, "name": tool.name,
                       "arguments": "\"北京\"}"})
                .to_string(),
                json!({"kind": "usage", "usage": {
                    "prompt_tokens": 11, "completion_tokens": 7, "reasoning_tokens": 3,
                    "cached_tokens": 5, "total_tokens": 18, "credit": 0.0
                }})
                .to_string(),
                json!({"kind": "finish", "reason": "tool_calls"}).to_string(),
            ];
        }

        vec![
            json!({"kind": "reasoning", "text": "（假渠道思考）"}).to_string(),
            json!({"kind": "text", "text": format!("[{}] ", request.model)}).to_string(),
            json!({"kind": "text", "text": echo}).to_string(),
            json!({"kind": "usage", "usage": {
                "prompt_tokens": 11, "completion_tokens": 7, "reasoning_tokens": 3,
                "cached_tokens": 5, "total_tokens": 18, "credit": 0.0
            }})
            .to_string(),
            json!({"kind": "finish", "reason": "stop"}).to_string(),
        ]
    }
}

#[async_trait]
impl ChannelAdapter for FakeChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_kind(&self) -> &str {
        "fake"
    }

    /// 假渠道不需要凭据：让"没有账号也能跑通链路"这件事成立。
    fn requires_credential(&self) -> bool {
        false
    }

    fn build_chat_request(
        &self,
        _account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<UpstreamRequest, GatewayError> {
        Ok(UpstreamRequest {
            url: "fake://chat".to_string(),
            headers: Vec::new(),
            body: request.raw.clone(),
        })
    }

    fn parse_event(&self, frame: &str) -> Vec<CanonicalEvent> {
        let Ok(value) = serde_json::from_str::<Value>(frame) else {
            return Vec::new();
        };
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(|text| vec![CanonicalEvent::Text { text: text.to_string() }])
                .unwrap_or_default()
        };
        match value.get("kind").and_then(Value::as_str).unwrap_or_default() {
            "text" => text("text"),
            "reasoning" => value
                .get("text")
                .and_then(Value::as_str)
                .map(|text| {
                    vec![CanonicalEvent::Reasoning {
                        text: text.to_string(),
                    }]
                })
                .unwrap_or_default(),
            "usage" => serde_json::from_value::<Usage>(value.get("usage").cloned().unwrap_or(Value::Null))
                .map(|usage| vec![CanonicalEvent::Usage(usage)])
                .unwrap_or_default(),
            "tool_call" => vec![CanonicalEvent::ToolCall {
                index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                id: value.get("id").and_then(Value::as_str).map(str::to_string),
                name: value.get("name").and_then(Value::as_str).map(str::to_string),
                arguments_delta: value
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }],
            "finish" => vec![CanonicalEvent::Finish {
                reason: value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string(),
            }],
            "error" => vec![CanonicalEvent::Error {
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("假渠道错误")
                    .to_string(),
            }],
            _ => Vec::new(),
        }
    }

    fn classify_error(&self, status: u16, body: &str, _upstream_model: &str) -> GatewayError {
        openai_compat::classify_openai_error(status, body)
    }

    async fn list_models(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Vec<UpstreamModel>>, GatewayError> {
        Ok(Some(vec![
            UpstreamModel {
                id: self.model.clone(),
                credits: Some("x0.00".into()),
                context_length: Some(1_000_000),
                max_output_tokens: Some(384_000),
                vendor: Some("fake".into()),
                capabilities: crate::gateway::channels::ModelCapabilities {
                    vision: false,
                    tool_calls: true,
                    reasoning: true,
                    // 假渠道的能力位是写死的，算"已声明"。
                    declared_tools: true,
                    declared_vision: true,
                },
                reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()],
                reasoning_default_effort: Some("high".into()),
                reasoning_can_disable: Some(true),
                ..UpstreamModel::default()
            },
            UpstreamModel {
                id: format!("{}-mini", self.model),
                ..UpstreamModel::default()
            },
        ]))
    }

    async fn chat_stream(
        &self,
        _account: &AccountSnapshot,
        request: &CanonicalRequest,
        _client: &reqwest::Client,
    ) -> Result<FrameStream, GatewayError> {
        let remaining = self.remaining_failures.load(Ordering::SeqCst);
        if remaining > 0 {
            self.remaining_failures.store(remaining - 1, Ordering::SeqCst);
            return Err(self.classify_error(
                self.fail_status,
                "fake injected failure",
                &request.model,
            ));
        }
        let frames: Vec<Result<String, GatewayError>> =
            self.frames(request).into_iter().map(Ok).collect();
        Ok(Box::pin(stream::iter(frames)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::cir::{Message, Role};

    fn request() -> CanonicalRequest {
        CanonicalRequest {
            model: "fake-model".into(),
            system: Vec::new(),
            messages: vec![Message {
                role: Role::User,
                content: vec![Block::text("你好")],
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                reasoning: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: true,
            reasoning: None,
            metadata: Default::default(),
            raw: json!({}),
        }
    }

    #[tokio::test]
    async fn frames_round_trip_through_parse_event() {
        let channel = FakeChannel::new("fake");
        let events: Vec<CanonicalEvent> = channel
            .frames(&request())
            .iter()
            .flat_map(|frame| channel.parse_event(frame))
            .collect();
        assert!(events
            .iter()
            .any(|event| matches!(event, CanonicalEvent::Text { text } if text == "你好")));
        assert!(events
            .iter()
            .any(|event| matches!(event, CanonicalEvent::Usage(usage) if usage.cached_tokens == 5)));
        assert!(events
            .iter()
            .any(|event| matches!(event, CanonicalEvent::Finish { reason } if reason == "stop")));
    }

    #[tokio::test]
    async fn injected_failure_happens_once_then_succeeds() {
        let channel = FakeChannel {
            remaining_failures: AtomicU32::new(1),
            ..FakeChannel::new("fake")
        };
        let client = reqwest::Client::new();
        let account = AccountSnapshot::default();
        let first = channel.chat_stream(&account, &request(), &client).await;
        assert!(first.is_err(), "第一次应当注入失败");
        assert!(channel.chat_stream(&account, &request(), &client).await.is_ok());
    }

    #[test]
    fn unknown_frame_is_ignored_not_fatal() {
        let channel = FakeChannel::new("fake");
        assert!(channel.parse_event("not json").is_empty());
        assert!(channel.parse_event(r#"{"kind":"future_thing"}"#).is_empty());
    }
}
