//! OpenAI Responses 请求 → CIR。
//!
//! Responses 与 chat 的三处硬差异：`input` 是 item 数组（含 `function_call` /
//! `function_call_output`）；`tools` 是扁平 function 形状（不套一层 `function`）；
//! `previous_response_id` 在无状态网关里必须直接拒绝。

use serde_json::Value;

use crate::gateway::errors::GatewayError;
use crate::gateway::protocol::cir::{
    Block, CanonicalRequest, Message, Reasoning, RequestMetadata, Role, ToolCall, ToolSpec,
};

pub fn decode_response(body: &Value) -> Result<CanonicalRequest, GatewayError> {
    let object = body
        .as_object()
        .ok_or_else(|| GatewayError::invalid_request("请求体必须是 JSON 对象"))?;

    // 网关无状态：拿不到上一轮的服务端上下文，只能明确拒绝而不是悄悄丢历史。
    if object.get("previous_response_id").is_some_and(|value| !value.is_null()) {
        return Err(GatewayError::invalid_request(
            "不支持 previous_response_id：本网关无状态，请把历史消息放进 input",
        ));
    }

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| GatewayError::invalid_request("缺少 model 字段"))?
        .to_string();

    let mut system = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        system.extend(text_blocks(instructions));
    }

    let mut messages = Vec::new();
    match object.get("input") {
        Some(Value::String(text)) => messages.push(user_message(text)),
        Some(Value::Array(items)) => {
            for item in items {
                decode_item(item, &mut messages, &mut system)?;
            }
        }
        Some(_) => return Err(GatewayError::invalid_request("input 必须是字符串或 item 数组")),
        None => return Err(GatewayError::invalid_request("缺少 input 字段")),
    }
    if messages.is_empty() {
        return Err(GatewayError::invalid_request("input 里没有可用消息"));
    }

    Ok(CanonicalRequest {
        model,
        system,
        messages,
        tools: decode_tools(object.get("tools")),
        // parallel_tool_calls / tool_choice 都透传，由渠道卫生管道落到上游字段。
        tool_choice: object.get("tool_choice").cloned(),
        temperature: object.get("temperature").and_then(Value::as_f64),
        top_p: object.get("top_p").and_then(Value::as_f64),
        max_tokens: object.get("max_output_tokens").and_then(Value::as_u64),
        stream: object.get("stream").and_then(Value::as_bool).unwrap_or(false),
        reasoning: decode_reasoning(object),
        metadata: RequestMetadata {
            user_id: object
                .get("user")
                .and_then(Value::as_str)
                .map(str::to_string),
            session_id: None,
            header_session: None,
        },
        raw: body.clone(),
    })
}

fn decode_item(
    item: &Value,
    messages: &mut Vec<Message>,
    system: &mut Vec<Block>,
) -> Result<(), GatewayError> {
    if let Some(text) = item.as_str() {
        messages.push(user_message(text));
        return Ok(());
    }
    let kind = item.get("type").and_then(Value::as_str);
    match kind {
        // 缺 type 时按 message 处理（input 里直接放 {role, content} 也常见）。
        None | Some("message") => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let blocks = text_blocks(item.get("content").unwrap_or(&Value::Null));
            match role {
                "system" | "developer" => system.extend(blocks),
                "assistant" => messages.push(Message {
                    role: Role::Assistant,
                    content: blocks,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                }),
                _ => messages.push(Message {
                    role: Role::User,
                    content: blocks,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                }),
            }
        }
        Some("function_call") => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = match item.get("arguments") {
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
                None => "{}".into(),
            };
            let call = ToolCall {
                id: item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("id").and_then(Value::as_str))
                    .map(str::to_string),
                name,
                arguments,
            };
            // 相邻的 function_call 合并进上一条 assistant，保持工具序列连续。
            if let Some(last) = messages.last_mut() {
                if last.role == Role::Assistant {
                    last.tool_calls.push(call);
                    return Ok(());
                }
            }
            messages.push(Message {
                role: Role::Assistant,
                content: Vec::new(),
                tool_calls: vec![call],
                tool_call_id: None,
                name: None,
                reasoning: None,
            });
        }
        Some("function_call_output") => {
            messages.push(Message {
                role: Role::Tool,
                content: match item.get("output") {
                    Some(Value::String(text)) => vec![Block::text(text.clone())],
                    Some(other) => vec![Block::text(other.to_string())],
                    None => Vec::new(),
                },
                tool_calls: Vec::new(),
                tool_call_id: item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                name: None,
                reasoning: None,
            });
        }
        // 历史思考项不回传（渠道卫生管道会按需回填空串）。
        Some("reasoning") => {}
        _ => {}
    }
    Ok(())
}

fn user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![Block::text(text)],
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        reasoning: None,
    }
}

/// content 的三种形状：字符串、`[{type:"input_text"|"output_text", text}]`、缺省。
fn text_blocks(content: &Value) -> Vec<Block> {
    match content {
        Value::String(text) => vec![Block::text(text.clone())],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(Block::text(text));
                }
                let kind = item.get("type").and_then(Value::as_str).unwrap_or("input_text");
                match kind {
                    "input_text" | "output_text" | "text" => {
                        item.get("text").and_then(Value::as_str).map(Block::text)
                    }
                    "input_image" => item
                        .get("image_url")
                        .and_then(Value::as_str)
                        .map(|url| Block::Image { url: url.to_string() }),
                    _ => None,
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn decode_tools(value: Option<&Value>) -> Vec<ToolSpec> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("function");
            // 服务端工具（web_search / web_fetch）在渠道层降级处理，这里先剔掉。
            if kind != "function" {
                return None;
            }
            let name = item.get("name").and_then(Value::as_str)?.to_string();
            Some(ToolSpec {
                name,
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                parameters: item.get("parameters").cloned(),
            })
        })
        .collect()
}

fn decode_reasoning(object: &serde_json::Map<String, Value>) -> Option<Reasoning> {
    let effort = object
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .or_else(|| object.get("reasoning_effort").and_then(Value::as_str))
        .map(str::to_string);
    effort.as_ref()?;
    Some(Reasoning {
        enabled: effort
            .as_deref()
            .map(|effort| !effort.eq_ignore_ascii_case("none")),
        effort,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(value: Value) -> CanonicalRequest {
        decode_response(&value).expect("解码成功")
    }

    #[test]
    fn string_input_becomes_one_user_message() {
        let request = decode(json!({"model": "glm-5.3", "input": "你好"}));
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].content[0].as_text(), Some("你好"));
    }

    #[test]
    fn instructions_become_system_and_max_output_tokens_is_unified() {
        let request = decode(json!({
            "model": "glm-5.3",
            "instructions": "你是助手",
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
            "max_output_tokens": 2048,
            "reasoning": {"effort": "high"}
        }));
        assert_eq!(request.system[0].as_text(), Some("你是助手"));
        assert_eq!(request.max_tokens, Some(2048));
        assert_eq!(
            request.reasoning.as_ref().and_then(|r| r.effort.clone()),
            Some("high".to_string())
        );
    }

    #[test]
    fn function_call_items_map_to_tool_calls_and_outputs() {
        let request = decode(json!({
            "model": "glm-5.3",
            "input": [
                {"type": "message", "role": "user", "content": "查天气"},
                {"type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"上海\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "晴"}
            ]
        }));
        assert_eq!(request.messages[1].role, Role::Assistant);
        assert_eq!(request.messages[1].tool_calls[0].name, "get_weather");
        assert_eq!(request.messages[2].role, Role::Tool);
        assert_eq!(request.messages[2].tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn adjacent_function_calls_merge_into_one_assistant_message() {
        let request = decode(json!({
            "model": "glm-5.3",
            "input": [
                {"type": "message", "role": "user", "content": "并行查"},
                {"type": "function_call", "call_id": "a", "name": "f1", "arguments": "{}"},
                {"type": "function_call", "call_id": "b", "name": "f2", "arguments": "{}"}
            ]
        }));
        assert_eq!(request.messages[1].tool_calls.len(), 2);
        assert_eq!(request.messages.len(), 2, "相邻 function_call 要合并");
    }

    #[test]
    fn previous_response_id_is_rejected_locally() {
        let error = decode_response(&json!({
            "model": "glm-5.3",
            "input": "hi",
            "previous_response_id": "resp_1"
        }))
        .expect_err("必须拒绝");
        assert_eq!(error.status, 400);
        assert!(error.message.contains("previous_response_id"));
    }

    #[test]
    fn server_side_tools_are_dropped_but_functions_kept() {
        let request = decode(json!({
            "model": "glm-5.3",
            "input": "hi",
            "tools": [
                {"type": "web_search"},
                {"type": "function", "name": "local_tool", "description": "本地", "parameters": {"type": "object"}}
            ]
        }));
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "local_tool");
    }
}
