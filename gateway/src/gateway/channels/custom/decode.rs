//! 上游 SSE → CIR（Anthropic Messages 事件流）。
//!
//! 帧名（`event:` 行）与负载里的 `type` 一致，所以直接读 `type`。
//! 未知事件返回空 vec —— 上游将来加字段不该中断整条流。

use serde_json::Value;

use crate::gateway::protocol::cir::{CanonicalEvent, Usage};

/// 一帧 Anthropic SSE → CIR 事件。
pub(super) fn parse_anthropic_event(frame: &str) -> Vec<CanonicalEvent> {
    let Ok(value) = serde_json::from_str::<Value>(frame) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str).unwrap_or_default() {
        "message_start" => {
            let usage = value
                .get("message")
                .and_then(|message| message.get("usage"));
            if let Some(usage) = usage {
                events.push(CanonicalEvent::Usage(anthropic_usage(usage)));
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let block = value.get("content_block").cloned().unwrap_or(Value::Null);
            match block.get("type").and_then(Value::as_str).unwrap_or_default() {
                "tool_use" => events.push(CanonicalEvent::ToolCall {
                    index,
                    id: block.get("id").and_then(Value::as_str).map(str::to_string),
                    name: block.get("name").and_then(Value::as_str).map(str::to_string),
                    arguments_delta: None,
                }),
                // 文本块起始为空占位；thinking 的正文在 delta 里。
                _ => {}
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let delta = value.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
                "text_delta" => {
                    if let Some(text) = delta
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        events.push(CanonicalEvent::Text {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking_delta" => {
                    if let Some(text) = delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        events.push(CanonicalEvent::Reasoning {
                            text: text.to_string(),
                        });
                    }
                }
                "input_json_delta" => {
                    if let Some(partial) = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                    {
                        events.push(CanonicalEvent::ToolCall {
                            index,
                            id: None,
                            name: None,
                            arguments_delta: Some(partial.to_string()),
                        });
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            if let Some(usage) = value.get("usage") {
                events.push(CanonicalEvent::Usage(anthropic_usage(usage)));
            }
            if let Some(reason) = value
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                events.push(CanonicalEvent::Finish {
                    reason: map_anthropic_stop_reason(reason),
                });
            }
        }
        "error" => {
            let message = value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游返回错误");
            events.push(CanonicalEvent::Error {
                message: message.to_string(),
            });
        }
        _ => {}
    }
    events
}

/// Anthropic usage → 内部用量。`input_tokens` 不含缓存读取量，要加上才是总输入。
fn anthropic_usage(usage: &Value) -> Usage {
    let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = number("cache_read_input_tokens");
    let prompt = number("input_tokens") + cached + number("cache_creation_input_tokens");
    let completion = number("output_tokens");
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        reasoning_tokens: 0,
        cached_tokens: cached,
        total_tokens: prompt + completion,
        credit: None,
    }
}

/// Anthropic 的 stop_reason 归一到 OpenAI 口径（下游按 OpenAI 名字解释）。
fn map_anthropic_stop_reason(reason: &str) -> String {
    match reason {
        "end_turn" | "stop_sequence" => "stop".into(),
        "max_tokens" => "length".into(),
        "tool_use" => "tool_calls".into(),
        other => other.to_string(),
    }
}


/// 一帧 OpenAI Responses SSE → CIR 事件（官方形状，对照 OpenAPI spec 核实）。
///
/// 官方流是带类型的事件序列，`type` **一定**在 data JSON 里（`event:` 行只是复述），
/// 所以只读 data。未知事件返回空 vec —— 官方有约 60 种事件类型，
/// 按需认领这几种就够还原完整输出。
pub(super) fn parse_responses_event(frame: &str) -> Vec<CanonicalEvent> {
    let Ok(value) = serde_json::from_str::<Value>(frame) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str).unwrap_or_default() {
        // 文本增量：`delta` 字段。
        "response.output_text.delta" => {
            if let Some(text) = value
                .get("delta")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                events.push(CanonicalEvent::Text {
                    text: text.to_string(),
                });
            }
        }
        // 思考增量：官方有两路 —— 正文推理（reasoning_text）与摘要推理
        // （reasoning_summary_text）。面板展示用摘要就够了，两路都收会重复。
        "response.reasoning_summary_text.delta" => {
            if let Some(text) = value
                .get("delta")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                events.push(CanonicalEvent::Reasoning {
                    text: text.to_string(),
                });
            }
        }
        // 工具调用：名字/ID 在 `output_item.added` 的 item 里，参数分片走
        // `response.function_call_arguments.delta`（注意不是 `.function_call.delta`）。
        "response.output_item.added" => {
            let item = value.get("item").cloned().unwrap_or(Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                events.push(CanonicalEvent::ToolCall {
                    index: value
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    id: item.get("call_id").and_then(Value::as_str).map(str::to_string),
                    name: item.get("name").and_then(Value::as_str).map(str::to_string),
                    arguments_delta: None,
                });
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = value
                .get("delta")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                events.push(CanonicalEvent::ToolCall {
                    index: value
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32,
                    id: None,
                    name: None,
                    arguments_delta: Some(delta.to_string()),
                });
            }
        }
        // 收尾：usage 在 `response.usage`（字段名与 chat 完全不同）。
        "response.completed" | "response.incomplete" => {
            if let Some(usage) = value
                .get("response")
                .and_then(|response| response.get("usage"))
            {
                events.push(CanonicalEvent::Usage(parse_responses_usage(usage)));
            }
            let reason = value
                .get("response")
                .and_then(|response| response.get("status"))
                .and_then(Value::as_str)
                .map(|status| if status == "incomplete" { "length" } else { "stop" })
                .unwrap_or("stop");
            events.push(CanonicalEvent::Finish {
                reason: reason.to_string(),
            });
        }
        "response.failed" => {
            let message = value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游返回 response.failed");
            events.push(CanonicalEvent::Error {
                message: message.to_string(),
            });
        }
        // 官方错误事件：`type` 就是 "error"（不在 response.* 命名空间里）。
        "error" => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("上游返回错误");
            events.push(CanonicalEvent::Error {
                message: message.to_string(),
            });
        }
        _ => {}
    }
    events
}

/// Responses usage → 内部用量：字段名与 chat 完全不同（input/output 而非 prompt/completion）。
fn parse_responses_usage(usage: &Value) -> Usage {
    let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let prompt = number("input_tokens");
    let completion = number("output_tokens");
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        reasoning_tokens: usage
            .get("output_tokens_details")
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_tokens: cached,
        total_tokens: number("total_tokens").max(prompt + completion),
        credit: None,
    }
}
