//! CIR 事件 → Anthropic SSE 事件序列与非流式 message。
//!
//! 事件序列固定：`message_start` → `content_block_start/delta/stop` → `message_delta` → `message_stop`。
//! content block 是**有状态**的：文本块与工具块都要显式 start/stop，所以这里做成一个编码器。

use std::collections::BTreeMap;

use axum::body::Bytes;
use serde_json::{json, Value};

use crate::gateway::protocol::cir::{CanonicalEvent, ToolCall, Usage};

/// stop_reason 映射：length → max_tokens、tool_calls → tool_use、内容过滤 → refusal、其余 → end_turn。
pub fn stop_reason(finish: &str) -> &'static str {
    match finish {
        "length" | "max_tokens" => "max_tokens",
        "tool_calls" | "tool_use" => "tool_use",
        "content_filter" => "refusal",
        "stop" | "end_turn" | "" => "end_turn",
        _ => "end_turn",
    }
}

pub fn event_frame(name: &str, payload: &Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {payload}\n\n"))
}

pub fn usage_json(usage: &Usage) -> Value {
    json!({
        "input_tokens": usage.prompt_tokens,
        "output_tokens": usage.completion_tokens,
        "cache_read_input_tokens": usage.cached_tokens,
        // 上游没有"缓存写入"口径，恒为 0（给出字段，客户端就不会报缺字段）。
        "cache_creation_input_tokens": 0,
    })
}

struct OpenTool {
    index: u32,
}

/// 有状态的流式编码器：负责 content block 的开合。
pub struct StreamEncoder {
    message_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    content_blocks: u32,
    text_block: Option<u32>,
    thinking_block: Option<u32>,
    tools: BTreeMap<u32, OpenTool>,
}

impl StreamEncoder {
    pub fn new(message_id: impl Into<String>, model: impl Into<String>, input_tokens: u64) -> Self {
        Self {
            message_id: message_id.into(),
            model: model.into(),
            input_tokens,
            output_tokens: 0,
            content_blocks: 0,
            text_block: None,
            thinking_block: None,
            tools: BTreeMap::new(),
        }
    }

    /// 开头：message_start（含 usage 的输入侧）。
    pub fn start(&self) -> Vec<Bytes> {
        vec![event_frame(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": { "input_tokens": self.input_tokens, "output_tokens": 0 },
                }
            }),
        )]
    }

    pub fn push(&mut self, event: &CanonicalEvent) -> Vec<Bytes> {
        match event {
            CanonicalEvent::Text { text } => {
                let mut frames = self.close_thinking();
                let index = match self.text_block {
                    Some(index) => index,
                    None => {
                        let index = self.next_index();
                        self.text_block = Some(index);
                        frames.push(event_frame(
                            "content_block_start",
                            &json!({ "type": "content_block_start", "index": index,
                                     "content_block": { "type": "text", "text": "" } }),
                        ));
                        index
                    }
                };
                frames.push(event_frame(
                    "content_block_delta",
                    &json!({ "type": "content_block_delta", "index": index,
                             "delta": { "type": "text_delta", "text": text } }),
                ));
                frames
            }
            CanonicalEvent::Reasoning { text } => {
                let mut frames = self.close_text();
                let index = match self.thinking_block {
                    Some(index) => index,
                    None => {
                        let index = self.next_index();
                        self.thinking_block = Some(index);
                        frames.push(event_frame(
                            "content_block_start",
                            &json!({ "type": "content_block_start", "index": index,
                                     "content_block": { "type": "thinking", "thinking": "" } }),
                        ));
                        index
                    }
                };
                frames.push(event_frame(
                    "content_block_delta",
                    &json!({ "type": "content_block_delta", "index": index,
                             "delta": { "type": "thinking_delta", "thinking": text } }),
                ));
                frames
            }
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let mut frames = self.close_text();
                frames.extend(self.close_thinking());
                let tool_index = match self.tools.get(index) {
                    Some(tool) => tool.index,
                    None => {
                        let position = self.next_index();
                        let tool_id = id.clone().unwrap_or_else(|| format!("toolu_{position}"));
                        frames.push(event_frame(
                            "content_block_start",
                            &json!({ "type": "content_block_start", "index": position,
                                     "content_block": { "type": "tool_use", "id": tool_id,
                                                        "name": name.clone().unwrap_or_default(),
                                                        "input": {} } }),
                        ));
                        self.tools.insert(*index, OpenTool { index: position });
                        position
                    }
                };
                if let Some(delta) = arguments_delta {
                    frames.push(event_frame(
                        "content_block_delta",
                        &json!({ "type": "content_block_delta", "index": tool_index,
                                 "delta": { "type": "input_json_delta", "partial_json": delta } }),
                    ));
                }
                frames
            }
            CanonicalEvent::Usage(usage) => {
                self.input_tokens = self.input_tokens.max(usage.prompt_tokens);
                self.output_tokens = self.output_tokens.max(usage.completion_tokens);
                Vec::new()
            }
            CanonicalEvent::Error { message } => vec![event_frame(
                "error",
                &json!({ "type": "error", "error": { "type": "api_error", "message": message } }),
            )],
            CanonicalEvent::Finish { .. } => Vec::new(),
        }
    }

    /// 收尾：补齐 content_block_stop，再发 message_delta + message_stop。
    pub fn finish(&mut self, finish: &str) -> Vec<Bytes> {
        let mut frames = self.close_text();
        frames.extend(self.close_thinking());
        for (_, tool) in std::mem::take(&mut self.tools) {
            frames.push(event_frame(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": tool.index }),
            ));
        }
        frames.push(event_frame(
            "message_delta",
            &json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_reason(finish), "stop_sequence": null },
                "usage": { "output_tokens": self.output_tokens },
            }),
        ));
        frames.push(event_frame(
            "message_stop",
            &json!({ "type": "message_stop" }),
        ));
        frames
    }

    fn next_index(&mut self) -> u32 {
        let index = self.content_blocks;
        self.content_blocks += 1;
        index
    }

    fn close_text(&mut self) -> Vec<Bytes> {
        match self.text_block.take() {
            Some(index) => vec![event_frame(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            )],
            None => Vec::new(),
        }
    }

    fn close_thinking(&mut self) -> Vec<Bytes> {
        match self.thinking_block.take() {
            Some(index) => vec![event_frame(
                "content_block_stop",
                &json!({ "type": "content_block_stop", "index": index }),
            )],
            None => Vec::new(),
        }
    }
}

/// 非流式：整条事件流 → 一个 Anthropic message。
pub fn fold(
    events: &[CanonicalEvent],
    message_id: &str,
    model: &str,
) -> Result<Value, crate::gateway::errors::GatewayError> {
    let mut content: Vec<Value> = Vec::new();
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tools: BTreeMap<u32, ToolCall> = BTreeMap::new();
    let mut finish = "stop".to_string();
    let mut usage = Usage::default();

    for event in events {
        match event {
            CanonicalEvent::Text { text: delta } => text.push_str(delta),
            CanonicalEvent::Reasoning { text: delta } => thinking.push_str(delta),
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let entry = tools.entry(*index).or_default();
                if entry.id.is_none() {
                    entry.id = id.clone();
                }
                if entry.name.is_empty() {
                    if let Some(name) = name {
                        entry.name = name.clone();
                    }
                }
                if let Some(delta) = arguments_delta {
                    entry.arguments.push_str(delta);
                }
            }
            CanonicalEvent::Usage(value) => usage = value.clone(),
            CanonicalEvent::Finish { reason } => finish = reason.clone(),
            CanonicalEvent::Error { message } => {
                return Err(crate::gateway::errors::GatewayError::upstream(502, message.clone()))
            }
        }
    }

    if !thinking.is_empty() {
        content.push(json!({ "type": "thinking", "thinking": thinking }));
    }
    if !text.is_empty() {
        content.push(json!({ "type": "text", "text": text }));
    }
    for (index, call) in tools {
        let input: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
        content.push(json!({
            "type": "tool_use",
            "id": call.id.unwrap_or_else(|| format!("toolu_{index}")),
            "name": call.name,
            "input": input,
        }));
    }

    Ok(json!({
        "id": message_id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason(&finish),
        "stop_sequence": null,
        "usage": usage_json(&usage),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames_text(frames: &[Bytes]) -> String {
        frames
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).to_string())
            .collect()
    }

    #[test]
    fn streaming_opens_and_closes_blocks_in_order() {
        let mut encoder = StreamEncoder::new("msg_1", "glm-5.3", 10);
        let mut out = frames_text(&encoder.start());
        out.push_str(&frames_text(&encoder.push(&CanonicalEvent::Text {
            text: "你好".into(),
        })));
        out.push_str(&frames_text(&encoder.push(&CanonicalEvent::ToolCall {
            index: 0,
            id: Some("toolu_9".into()),
            name: Some("get_weather".into()),
            arguments_delta: Some("{\"city\"".into()),
        })));
        out.push_str(&frames_text(&encoder.finish("tool_calls")));

        assert!(out.contains("event: message_start"));
        assert!(out.contains("\"type\":\"content_block_start\""), "{out}");
        assert!(out.contains("\"type\":\"text_delta\""), "{out}");
        assert!(out.contains("你好"), "{out}");
        assert!(
            out.contains("\"type\":\"input_json_delta\""),
            "缺 input_json_delta：{out}"
        );
        assert!(out.contains("\"stop_reason\":\"tool_use\""), "{out}");
        assert!(out.contains("event: message_stop"));
        // 文本块在工具块开始前必须关掉。
        let text_stop = out.find("content_block_stop").expect("有关闭");
        let tool_start = out.find("tool_use").expect("有工具块");
        assert!(text_stop < tool_start, "{out}");
    }

    #[test]
    fn stop_reason_mapping_covers_the_documented_cases() {
        assert_eq!(stop_reason("length"), "max_tokens");
        assert_eq!(stop_reason("tool_calls"), "tool_use");
        assert_eq!(stop_reason("content_filter"), "refusal");
        assert_eq!(stop_reason("stop"), "end_turn");
        assert_eq!(stop_reason(""), "end_turn");
    }

    #[test]
    fn non_streaming_fold_builds_message_with_tool_use_and_usage() {
        let events = vec![
            CanonicalEvent::Reasoning {
                text: "想一想".into(),
            },
            CanonicalEvent::Text { text: "查一下".into() },
            CanonicalEvent::ToolCall {
                index: 0,
                id: Some("toolu_1".into()),
                name: Some("get_weather".into()),
                arguments_delta: Some("{\"city\":\"上海\"}".into()),
            },
            CanonicalEvent::Usage(Usage {
                prompt_tokens: 12,
                completion_tokens: 5,
                cached_tokens: 8,
                total_tokens: 17,
                ..Usage::default()
            }),
            CanonicalEvent::Finish {
                reason: "tool_calls".into(),
            },
        ];
        let message = fold(&events, "msg_2", "glm-5.3").expect("折叠成功");
        assert_eq!(message["type"], "message");
        assert_eq!(message["content"][0]["type"], "thinking");
        assert_eq!(message["content"][1]["text"], "查一下");
        assert_eq!(message["content"][2]["type"], "tool_use");
        assert_eq!(message["content"][2]["input"]["city"], "上海");
        assert_eq!(message["stop_reason"], "tool_use");
        assert_eq!(message["usage"]["input_tokens"], 12);
        assert_eq!(message["usage"]["cache_read_input_tokens"], 8);
    }
}
