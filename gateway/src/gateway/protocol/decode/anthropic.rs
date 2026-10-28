//! Anthropic Messages 请求 → CIR。
//!
//! 映射要点（与计划 §7.5 一致）：`system` 提到 system 块；`tool_use` 块变成 assistant 的
//! `tool_calls`；`tool_result` 块拆成独立的 tool 消息；`thinking`/`redacted_thinking` 丢弃；
//! `document` 降级为文本；服务端工具（web_search 等）跳过并在 system 末尾说明不可用。

use serde_json::Value;

use crate::gateway::errors::GatewayError;
use crate::gateway::protocol::cir::{
    Block, CanonicalRequest, Message, Reasoning, RequestMetadata, Role, ToolCall, ToolSpec,
};

/// 服务端工具：上游没有对应实现，明确告诉模型"别调"。
const SERVER_TOOLS: [&str; 4] = ["web_search", "web_fetch", "computer", "bash"];

pub fn decode_messages(body: &Value) -> Result<CanonicalRequest, GatewayError> {
    let object = body
        .as_object()
        .ok_or_else(|| GatewayError::invalid_request("请求体必须是 JSON 对象"))?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| GatewayError::invalid_request("缺少 model 字段"))?
        .to_string();

    let mut system = decode_system(object.get("system"));
    let (tools, skipped) = decode_tools(object.get("tools"));
    if !skipped.is_empty() {
        system.push(Block::text(format!(
            "以下工具在当前环境中不可用，请不要调用：{}",
            skipped.join("、")
        )));
    }

    let mut messages = Vec::new();
    if let Some(list) = object.get("messages").and_then(Value::as_array) {
        for item in list {
            decode_message(item, &mut messages)?;
        }
    }
    if messages.is_empty() {
        return Err(GatewayError::invalid_request("messages 为空"));
    }

    Ok(CanonicalRequest {
        model,
        system,
        messages,
        tools,
        tool_choice: decode_tool_choice(object.get("tool_choice"), object),
        temperature: object.get("temperature").and_then(Value::as_f64),
        top_p: object.get("top_p").and_then(Value::as_f64),
        max_tokens: object.get("max_tokens").and_then(Value::as_u64),
        stream: object.get("stream").and_then(Value::as_bool).unwrap_or(false),
        reasoning: decode_thinking(object.get("thinking")),
        metadata: RequestMetadata {
            user_id: object
                .get("metadata")
                .and_then(|metadata| metadata.get("user_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            session_id: None,
            header_session: None,
        },
        raw: body.clone(),
    })
}

fn decode_system(value: Option<&Value>) -> Vec<Block> {
    match value {
        Some(Value::String(text)) => vec![Block::text(text.clone())],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text").and_then(Value::as_str).map(Block::text)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn decode_message(item: &Value, messages: &mut Vec<Message>) -> Result<(), GatewayError> {
    let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    let assistant = role == "assistant";

    let Some(content) = item.get("content") else {
        return Ok(());
    };
    // content 允许是纯字符串。
    if let Some(text) = content.as_str() {
        messages.push(Message {
            role: if assistant { Role::Assistant } else { Role::User },
            content: vec![Block::text(text)],
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        });
        return Ok(());
    }

    let Some(blocks) = content.as_array() else {
        return Err(GatewayError::invalid_request("content 必须是字符串或块数组"));
    };

    let mut text_blocks = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning: Option<String> = None;

    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("text") {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_blocks.push(Block::text(text));
                }
            }
            "image" => {
                if let Some(url) = decode_image(block) {
                    text_blocks.push(Block::Image { url });
                }
            }
            "tool_use" => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = block
                    .get("input")
                    .map(|input| input.to_string())
                    .unwrap_or_else(|| "{}".into());
                tool_calls.push(ToolCall {
                    id: block.get("id").and_then(Value::as_str).map(str::to_string),
                    name,
                    arguments,
                });
            }
            "tool_result" => {
                // 工具结果要拆成独立消息，并且必须排在对应的 assistant 之后。
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                messages.push(Message {
                    role: Role::Tool,
                    content: decode_tool_result_content(block.get("content")),
                    tool_calls: Vec::new(),
                    tool_call_id: call_id,
                    name: None,
                    reasoning: None,
                });
            }
            // 思考块：本轮不回传（上游对历史 reasoning 的要求由渠道卫生管道处理）。
            "thinking" => {
                reasoning = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            "redacted_thinking" => {}
            // 文档块降级成文本说明，避免整条请求失败。
            "document" => {
                let title = block
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("document");
                text_blocks.push(Block::text(format!("[文档 {title} 已省略]")));
            }
            _ => {}
        }
    }

    if !text_blocks.is_empty() || !tool_calls.is_empty() || reasoning.is_some() {
        messages.push(Message {
            role: if assistant { Role::Assistant } else { Role::User },
            content: text_blocks,
            tool_calls,
            tool_call_id: None,
            name: None,
            reasoning,
        });
    }
    Ok(())
}

fn decode_image(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    match source.get("type").and_then(Value::as_str) {
        Some("base64") => {
            let media = source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png");
            let data = source.get("data").and_then(Value::as_str)?;
            Some(format!("data:{media};base64,{data}"))
        }
        Some("url") => source
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn decode_tool_result_content(content: Option<&Value>) -> Vec<Block> {
    match content {
        Some(Value::String(text)) => vec![Block::text(text.clone())],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str).map(Block::text))
            .collect(),
        _ => Vec::new(),
    }
}

fn decode_tools(value: Option<&Value>) -> (Vec<ToolSpec>, Vec<String>) {
    let mut tools = Vec::new();
    let mut skipped = Vec::new();
    for item in value.and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        if SERVER_TOOLS.contains(&name) {
            skipped.push(name.to_string());
            continue;
        }
        tools.push(ToolSpec {
            name: name.to_string(),
            description: item
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            parameters: item.get("input_schema").cloned(),
        });
    }
    (tools, skipped)
}

/// `tool_choice.type=any` → `required`；指定具体工具 → OpenAI 的 function 形状。
fn decode_tool_choice(value: Option<&Value>, object: &serde_json::Map<String, Value>) -> Option<Value> {
    let parallel = object
        .get("tool_choice")
        .and_then(|choice| choice.get("disable_parallel_tool_use"))
        .and_then(Value::as_bool);
    let choice = match value?.get("type").and_then(Value::as_str) {
        Some("any") => serde_json::json!("required"),
        Some("tool") => {
            let name = value?.get("name").and_then(Value::as_str)?;
            serde_json::json!({ "type": "function", "function": { "name": name } })
        }
        Some("none") => serde_json::json!("none"),
        _ => serde_json::json!("auto"),
    };
    // 并行调用开关交给渠道适配器在请求体里落成 parallel_tool_calls。
    if parallel == Some(true) {
        Some(choice)
    } else {
        Some(choice)
    }
}

/// 思考预算 → 档位（上游只认 low/medium/high）。
fn decode_thinking(value: Option<&Value>) -> Option<Reasoning> {
    let thinking = value?;
    let kind = thinking.get("type").and_then(Value::as_str).unwrap_or("disabled");
    if kind != "enabled" {
        return Some(Reasoning {
            enabled: Some(false),
            effort: None,
        });
    }
    let budget = thinking
        .get("budget_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(8_000);
    let effort = if budget < 4_000 {
        "low"
    } else if budget < 16_000 {
        "medium"
    } else {
        "high"
    };
    Some(Reasoning {
        enabled: Some(true),
        effort: Some(effort.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(value: Value) -> CanonicalRequest {
        decode_messages(&value).expect("解码成功")
    }

    #[test]
    fn system_string_and_tool_blocks_are_mapped() {
        let request = decode(json!({
            "model": "glm-5.3",
            "max_tokens": 1024,
            "system": [{"type": "text", "text": "你是助手"}],
            "messages": [{"role": "user", "content": "你好"}],
            "tools": [{"name": "get_weather", "description": "查天气", "input_schema": {"type": "object"}}]
        }));
        assert_eq!(request.system[0].as_text(), Some("你是助手"));
        assert_eq!(request.tools[0].name, "get_weather");
        assert_eq!(request.max_tokens, Some(1024));
        assert_eq!(request.messages[0].role, Role::User);
    }

    #[test]
    fn tool_use_and_tool_result_become_tool_calls_and_tool_messages() {
        let request = decode(json!({
            "model": "glm-5.3",
            "messages": [
                {"role": "user", "content": "查天气"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "我来查"},
                    {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "上海"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "晴 26 度"}
                ]}
            ]
        }));
        let assistant = &request.messages[1];
        assert_eq!(assistant.role, Role::Assistant);
        assert_eq!(assistant.tool_calls[0].name, "get_weather");
        assert!(assistant.tool_calls[0].arguments.contains("上海"));
        let tool = &request.messages[2];
        assert_eq!(tool.role, Role::Tool);
        assert_eq!(tool.tool_call_id.as_deref(), Some("toolu_1"));
        assert_eq!(tool.content[0].as_text(), Some("晴 26 度"));
    }

    #[test]
    fn images_and_documents_are_normalised() {
        let request = decode(json!({
            "model": "glm-5v-turbo",
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AAA"}},
                {"type": "document", "title": "spec.md"}
            ]}]
        }));
        let blocks = &request.messages[0].content;
        assert!(matches!(&blocks[0], Block::Image { url } if url.starts_with("data:image/jpeg;base64,")));
        assert!(blocks[1].as_text().unwrap().contains("spec.md"));
    }

    #[test]
    fn server_tools_are_skipped_with_a_note_in_system() {
        let request = decode(json!({
            "model": "glm-5.3",
            "system": "系统",
            "messages": [{"role": "user", "content": "搜一下"}],
            "tools": [{"name": "web_search"}, {"name": "local_tool", "input_schema": {}}]
        }));
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "local_tool");
        let note = request.system.last().unwrap().as_text().unwrap();
        assert!(note.contains("web_search"), "{note}");
    }

    #[test]
    fn thinking_budget_maps_to_effort_tiers() {
        let low = decode(json!({
            "model": "deepseek-v4-pro",
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "enabled", "budget_tokens": 2000}
        }));
        assert_eq!(low.reasoning.unwrap().effort.as_deref(), Some("low"));

        let high = decode(json!({
            "model": "deepseek-v4-pro",
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "enabled", "budget_tokens": 32000}
        }));
        assert_eq!(high.reasoning.unwrap().effort.as_deref(), Some("high"));

        let off = decode(json!({
            "model": "deepseek-v4-pro",
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "disabled"}
        }));
        assert_eq!(off.reasoning.unwrap().enabled, Some(false));
    }

    #[test]
    fn tool_choice_any_becomes_required() {
        let request = decode(json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
            "tool_choice": {"type": "any"}
        }));
        assert_eq!(request.tool_choice, Some(json!("required")));

        let request = decode(json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "hi"}],
            "tool_choice": {"type": "tool", "name": "get_weather"}
        }));
        assert_eq!(
            request.tool_choice,
            Some(json!({"type": "function", "function": {"name": "get_weather"}}))
        );
    }
}
