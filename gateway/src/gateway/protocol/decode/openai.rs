//! OpenAI Chat Completions（含旧版 `/v1/completions` 的 `prompt`）→ CIR。
//!
//! 解码只做**归一**，不做渠道适配：`developer` 归一到 system、`max_completion_tokens`
//! 归一到 `max_tokens`、content 的字符串/块数组两种写法都收。渠道特有的改写
//! （指纹清洗、思考注入、强制流式）在渠道适配器里做。

use serde_json::Value;

use crate::gateway::errors::GatewayError;
use crate::gateway::protocol::cir::{
    Block, CanonicalRequest, Message, Reasoning, RequestMetadata, Role, ToolCall, ToolSpec,
};

pub fn decode_chat(body: &Value) -> Result<CanonicalRequest, GatewayError> {
    let object = body
        .as_object()
        .ok_or_else(|| GatewayError::invalid_request("请求体必须是 JSON 对象"))?;

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| GatewayError::invalid_request("缺少 model 字段"))?
        .to_string();

    let mut system = Vec::new();
    let mut messages = Vec::new();
    if let Some(list) = object.get("messages").and_then(Value::as_array) {
        for item in list {
            decode_message(item, &mut system, &mut messages)?;
        }
    } else if let Some(prompt) = object.get("prompt") {
        messages.push(Message {
            role: Role::User,
            content: vec![Block::text(decode_prompt(prompt))],
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        });
    } else {
        return Err(GatewayError::invalid_request("缺少 messages 字段"));
    }

    if messages.is_empty() && system.is_empty() {
        return Err(GatewayError::invalid_request("messages 为空"));
    }

    Ok(CanonicalRequest {
        model,
        system,
        messages,
        tools: decode_tools(object.get("tools")),
        tool_choice: object.get("tool_choice").cloned(),
        temperature: object.get("temperature").and_then(Value::as_f64),
        top_p: object.get("top_p").and_then(Value::as_f64),
        max_tokens: object
            .get("max_tokens")
            .and_then(Value::as_u64)
            .or_else(|| object.get("max_completion_tokens").and_then(Value::as_u64)),
        stream: object.get("stream").and_then(Value::as_bool).unwrap_or(false),
        reasoning: decode_reasoning(object),
        metadata: decode_metadata(object),
        raw: body.clone(),
    })
}

fn decode_message(
    item: &Value,
    system: &mut Vec<Block>,
    messages: &mut Vec<Message>,
) -> Result<(), GatewayError> {
    let raw_role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    // 上游只认 system/user/assistant/tool：developer 是 OpenAI 新名，归一到 system。
    let role = if raw_role == "developer" { "system" } else { raw_role };
    let content = decode_content(item.get("content"))?;

    match role {
        "system" => system.extend(content),
        "assistant" => messages.push(Message {
            role: Role::Assistant,
            content,
            tool_calls: decode_tool_calls(item.get("tool_calls")),
            tool_call_id: None,
            name: None,
            // 历史思考内容原样保留（deepseek 系上游要求它在开启思考时非空）。
            reasoning: item
                .get("reasoning_content")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        "tool" => messages.push(Message {
            role: Role::Tool,
            content,
            tool_calls: Vec::new(),
            tool_call_id: item
                .get("tool_call_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            name: item.get("name").and_then(Value::as_str).map(str::to_string),
            reasoning: None,
        }),
        _ => messages.push(Message {
            role: Role::User,
            content,
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        }),
    }
    Ok(())
}

/// content 的三种形状：字符串、块数组、缺省。
fn decode_content(value: Option<&Value>) -> Result<Vec<Block>, GatewayError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![Block::text(text.clone())]),
        Value::Array(parts) => {
            let mut blocks = Vec::new();
            for part in parts {
                match part {
                    Value::String(text) => blocks.push(Block::text(text.clone())),
                    Value::Object(map) => {
                        let kind = map.get("type").and_then(Value::as_str).unwrap_or("text");
                        match kind {
                            "text" | "input_text" | "output_text" => {
                                if let Some(text) = map.get("text").and_then(Value::as_str) {
                                    blocks.push(Block::text(text));
                                }
                            }
                            "image_url" | "input_image" => {
                                if let Some(url) = image_url(map.get("image_url")) {
                                    blocks.push(Block::Image { url });
                                }
                            }
                            // 未知块类型忽略，不因为多一个字段就拒绝整个请求。
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Ok(blocks)
        }
        _ => Err(GatewayError::invalid_request("content 必须是字符串或块数组")),
    }
}

fn image_url(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(url) => Some(url.clone()),
        Value::Object(map) => map.get("url").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// 旧版 completions 的 `prompt`：字符串或字符串数组（数组按换行拼接）。
fn decode_prompt(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn decode_tool_calls(value: Option<&Value>) -> Vec<ToolCall> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let function = item.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?.to_string();
            let arguments = match function.get("arguments") {
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            Some(ToolCall {
                id: item.get("id").and_then(Value::as_str).map(str::to_string),
                name,
                arguments,
            })
        })
        .collect()
}

fn decode_tools(value: Option<&Value>) -> Vec<ToolSpec> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            // 只认 function 工具；服务端工具（web_search 等）在渠道适配器里降级处理。
            let function = item.get("function")?;
            let name = function.get("name").and_then(Value::as_str)?.to_string();
            Some(ToolSpec {
                name,
                description: function
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                parameters: function.get("parameters").cloned(),
            })
        })
        .collect()
}

fn decode_reasoning(object: &serde_json::Map<String, Value>) -> Option<Reasoning> {
    let effort = object
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            object
                .get("reasoning")
                .and_then(|value| value.get("effort"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let mut enabled = effort
        .as_deref()
        .map(|effort| !effort.eq_ignore_ascii_case("none"));
    // 显式关闭思考的写法（DeepSeek 风格的 thinking 开关）。
    if object
        .get("thinking")
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("disabled"))
    {
        enabled = Some(false);
    }
    if effort.is_none() && enabled.is_none() {
        return None;
    }
    Some(Reasoning { enabled, effort })
}

fn decode_metadata(object: &serde_json::Map<String, Value>) -> RequestMetadata {
    let nested = object.get("metadata").and_then(Value::as_object);
    RequestMetadata {
        user_id: nested
            .and_then(|map| map.get("user_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        session_id: ["conversation_id", "session_id", "prompt_cache_key"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .or_else(|| {
                nested
                    .and_then(|map| map.get("session_id"))
                    .and_then(Value::as_str)
            })
            .map(str::to_string),
        header_session: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(value: Value) -> CanonicalRequest {
        decode_chat(&value).expect("解码应当成功")
    }

    #[test]
    fn developer_role_folds_into_system_and_user_stays() {
        let request = decode(json!({
            "model": "glm-5.3",
            "messages": [
                {"role": "developer", "content": "be brief"},
                {"role": "system", "content": "second"},
                {"role": "user", "content": "你好"}
            ]
        }));
        assert_eq!(request.system.len(), 2);
        assert_eq!(request.system[0].as_text(), Some("be brief"));
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, Role::User);
    }

    #[test]
    fn content_blocks_and_tool_calls_are_normalised() {
        let request = decode(json!({
            "model": "hy4-preview-f",
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "看图"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
                ]},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"上海\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "晴"}
            ]
        }));
        assert!(matches!(request.messages[0].content[1], Block::Image { .. }));
        assert_eq!(request.messages[1].tool_calls[0].name, "get_weather");
        assert_eq!(request.messages[2].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(request.messages[2].role, Role::Tool);
    }

    #[test]
    fn max_completion_tokens_and_reasoning_effort_are_unified() {
        let request = decode(json!({
            "model": "deepseek-v4.1-flash",
            "messages": [{"role": "user", "content": "hi"}],
            "max_completion_tokens": 4096,
            "reasoning_effort": "high",
            "stream": true,
            "temperature": 0.2
        }));
        assert_eq!(request.max_tokens, Some(4096));
        assert!(request.stream);
        assert_eq!(
            request.reasoning.as_ref().and_then(|r| r.effort.clone()),
            Some("high".to_string())
        );
        assert_eq!(request.temperature, Some(0.2));
    }

    #[test]
    fn thinking_disabled_wins_over_effort() {
        let request = decode(json!({
            "model": "deepseek-v4-pro",
            "messages": [{"role": "user", "content": "hi"}],
            "reasoning_effort": "high",
            "thinking": {"type": "disabled"}
        }));
        let reasoning = request.reasoning.expect("有思考配置");
        assert_eq!(reasoning.enabled, Some(false));
    }

    #[test]
    fn legacy_prompt_becomes_user_message() {
        let request = decode(json!({"model": "hy3", "prompt": ["第一行", "第二行"]}));
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].content[0].as_text(), Some("第一行\n第二行"));
    }

    #[test]
    fn missing_model_is_rejected_locally() {
        let error = decode_chat(&json!({"messages": [{"role": "user", "content": "hi"}]}))
            .expect_err("缺 model 必须本地 400");
        assert_eq!(error.status, 400);
        assert!(!error.retryable);
    }
}
