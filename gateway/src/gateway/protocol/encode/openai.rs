//! CIR 事件 → OpenAI Chat Completions 的 SSE 帧与非流式折叠。

use std::collections::BTreeMap;

use axum::body::Bytes;
use serde_json::{json, Value};

use crate::gateway::errors::GatewayError;
use crate::gateway::protocol::cir::{CanonicalEvent, ToolCall, Usage};

/// 上游 usage 里的缓存字段在各客户端读法不同，命中值统一写进所有别名。
pub fn usage_json(usage: &Usage) -> Value {
    let mut value = json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.total_tokens,
        "prompt_tokens_details": { "cached_tokens": usage.cached_tokens },
        "completion_tokens_details": { "reasoning_tokens": usage.reasoning_tokens },
        "cached_tokens": usage.cached_tokens,
        "prompt_cache_hit_tokens": usage.cached_tokens,
        "cache_read_input_tokens": usage.cached_tokens,
    });
    if let (Some(credit), Some(map)) = (usage.credit, value.as_object_mut()) {
        map.insert("credit".into(), json!(credit));
    }
    value
}

/// 单个事件 → 一个 streaming chunk。返回 None 表示该事件不产生帧。
pub fn chunk(event: &CanonicalEvent, id: &str, model: &str, created: u64) -> Option<Value> {
    let with_delta = |delta: Value, finish: Option<&str>| {
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        })
    };
    Some(match event {
        CanonicalEvent::Text { text } => with_delta(json!({ "content": text }), None),
        CanonicalEvent::Reasoning { text } => with_delta(json!({ "reasoning_content": text }), None),
        CanonicalEvent::ToolCall {
            index,
            id,
            name,
            arguments_delta,
        } => {
            let mut function = serde_json::Map::new();
            if let Some(name) = name {
                function.insert("name".into(), json!(name));
            }
            if let Some(delta) = arguments_delta {
                function.insert("arguments".into(), json!(delta));
            }
            let mut call = serde_json::Map::new();
            call.insert("index".into(), json!(index));
            if let Some(call_id) = id {
                call.insert("id".into(), json!(call_id));
                call.insert("type".into(), json!("function"));
            }
            call.insert("function".into(), Value::Object(function));
            with_delta(json!({ "tool_calls": [Value::Object(call)] }), None)
        }
        CanonicalEvent::Usage(usage) => json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": usage_json(usage),
        }),
        CanonicalEvent::Finish { reason } => with_delta(json!({}), Some(reason)),
        CanonicalEvent::Error { message } => json!({
            "error": { "message": message, "type": "api_error" }
        }),
    })
}

/// 非流式：把整条事件流折叠成一个 `chat.completion`。
pub fn fold(
    events: &[CanonicalEvent],
    id: &str,
    model: &str,
    created: u64,
) -> Result<Value, GatewayError> {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: BTreeMap<u32, ToolCall> = BTreeMap::new();
    let mut finish = "stop".to_string();
    let mut usage = Usage::default();

    for event in events {
        match event {
            CanonicalEvent::Text { text } => content.push_str(text),
            CanonicalEvent::Reasoning { text } => reasoning.push_str(text),
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let entry = tool_calls.entry(*index).or_default();
                if entry.id.is_none() {
                    entry.id = id.clone();
                }
                if let Some(name) = name {
                    if entry.name.is_empty() {
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
                return Err(GatewayError::upstream(502, message.clone()));
            }
        }
    }

    let mut message = serde_json::Map::new();
    message.insert("role".into(), json!("assistant"));
    message.insert("content".into(), json!(content));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".into(), json!(reasoning));
    }
    if !tool_calls.is_empty() {
        let calls: Vec<Value> = tool_calls
            .into_values()
            .map(|call| {
                json!({
                    "id": call.id.unwrap_or_else(|| "call_0".into()),
                    "type": "function",
                    "function": { "name": call.name, "arguments": call.arguments },
                })
            })
            .collect();
        message.insert("tool_calls".into(), json!(calls));
        if finish == "stop" {
            finish = "tool_calls".into();
        }
    }

    Ok(json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "message": Value::Object(message), "finish_reason": finish }],
        "usage": usage_json(&usage),
    }))
}

pub fn sse_data(value: &Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

pub fn sse_done() -> Bytes {
    Bytes::from_static(b"data: [DONE]\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_chunks_carry_delta_and_finish() {
        let text = chunk(&CanonicalEvent::Text { text: "你".into() }, "id", "m", 1)
            .expect("文本事件应当出帧");
        assert_eq!(text["choices"][0]["delta"]["content"], "你");
        assert!(text["choices"][0]["finish_reason"].is_null());

        let finish = chunk(&CanonicalEvent::Finish { reason: "stop".into() }, "id", "m", 1)
            .expect("结束事件应当出帧");
        assert_eq!(finish["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn fold_aggregates_tool_call_arguments_by_index() {
        let events = vec![
            CanonicalEvent::Text { text: "看".into() },
            CanonicalEvent::ToolCall {
                index: 0,
                id: Some("call_1".into()),
                name: Some("get_weather".into()),
                arguments_delta: Some("{\"city\"".into()),
            },
            CanonicalEvent::ToolCall {
                index: 0,
                id: None,
                name: None,
                arguments_delta: Some(":\"上海\"}".into()),
            },
            CanonicalEvent::Usage(Usage {
                prompt_tokens: 12,
                completion_tokens: 3,
                cached_tokens: 8,
                total_tokens: 15,
                ..Usage::default()
            }),
            CanonicalEvent::Finish { reason: "stop".into() },
        ];
        let completion = fold(&events, "chatcmpl-1", "glm-5.3", 7).expect("折叠成功");
        let message = &completion["choices"][0]["message"];
        assert_eq!(message["content"], "看");
        assert_eq!(
            message["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"上海\"}"
        );
        // 有工具调用时 finish_reason 归一为 tool_calls。
        assert_eq!(completion["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(completion["usage"]["prompt_tokens"], 12);
        assert_eq!(completion["usage"]["prompt_cache_hit_tokens"], 8);
    }

    #[test]
    fn fold_surfaces_upstream_error_instead_of_fake_completion() {
        let events = vec![CanonicalEvent::Error {
            message: "上游断流".into(),
        }];
        let error = fold(&events, "id", "m", 0).expect_err("必须报错");
        assert_eq!(error.status, 502);
    }
}
