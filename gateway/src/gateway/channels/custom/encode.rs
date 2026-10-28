//! CIR → 上游报文（OpenAI Chat Completions 与 Anthropic Messages 两套）。
//!
//! 共同点：以客户端原文为底（保留 `top_p`、`stop`、`stream_options` 这类透传字段），
//! 再重建消息与工具。客户端可能是**另一种**协议（Claude Code 指过来时是 Anthropic），
//! 它的顶层 `system` 必须按目标协议重新安放。

use serde_json::{json, Value};

use crate::gateway::protocol::cir::{Block, CanonicalRequest, Message, Role};

/// CIR → OpenAI Chat Completions 报文。
pub(crate) fn build_openai_body(request: &CanonicalRequest) -> Value {
    let mut body = match request.raw.clone() {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    if let Some(map) = body.as_object_mut() {
        // 内部标记键不下发；客户端协议的顶层提示词字段已被重建进 messages，删掉免得发两份。
        map.retain(|key, _| !key.starts_with('_'));
        map.remove("system");
        map.remove("instructions");
        map.remove("input");
        map.remove("max_completion_tokens");
    }

    let mut messages = Vec::new();
    let system: Vec<&str> = request.system.iter().filter_map(Block::as_text).collect();
    if !system.is_empty() {
        messages.push(json!({ "role": "system", "content": system.join("\n") }));
    }
    messages.extend(request.messages.iter().map(openai_message));
    body["messages"] = Value::Array(messages);
    body["model"] = json!(request.model);

    if request.tools.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.remove("tools");
        }
    } else {
        body["tools"] = json!(request
            .tools
            .iter()
            .map(|tool| {
                let mut function = json!({ "name": tool.name });
                if let Some(description) = &tool.description {
                    function["description"] = json!(description);
                }
                if let Some(parameters) = &tool.parameters {
                    function["parameters"] = parameters.clone();
                }
                json!({ "type": "function", "function": function })
            })
            .collect::<Vec<_>>());
    }

    if let Some(choice) = &request.tool_choice {
        if let Some(normalized) = normalize_tool_choice(choice) {
            body["tool_choice"] = normalized;
        }
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    body["stream"] = json!(true);
    body
}

fn openai_message(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let texts: Vec<&str> = message.content.iter().filter_map(Block::as_text).collect();
    let images: Vec<&str> = message
        .content
        .iter()
        .filter_map(|block| match block {
            Block::Image { url } => Some(url.as_str()),
            Block::Text { .. } => None,
        })
        .collect();

    let content = if images.is_empty() {
        Value::String(texts.join("\n"))
    } else {
        let mut parts = Vec::new();
        for text in texts {
            parts.push(json!({ "type": "text", "text": text }));
        }
        for url in images {
            parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
        }
        Value::Array(parts)
    };

    let mut value = json!({ "role": role, "content": content });
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = json!(message
            .tool_calls
            .iter()
            .map(|call| json!({
                "id": call.id.clone().unwrap_or_default(),
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments },
            }))
            .collect::<Vec<_>>());
    }
    if let Some(id) = &message.tool_call_id {
        value["tool_call_id"] = json!(id);
    }
    if let Some(reasoning) = &message.reasoning {
        value["reasoning_content"] = json!(reasoning);
    }
    value
}

/// tool_choice 归一到 OpenAI 形状（对象或三字符串之一）。
///
/// 入参可能是 Anthropic 形状（解码层原样透传）：`{type:"tool",name}`、
/// `{type:"any"}`、`{type:"auto"}`。直接下发会让 OpenAI 面服务端拒收。
pub(super) fn normalize_tool_choice(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(text) => {
            let lowered = text.trim().to_ascii_lowercase();
            matches!(lowered.as_str(), "auto" | "required" | "none").then(|| json!(lowered))
        }
        Value::Object(map) => {
            match map.get("type").and_then(Value::as_str).unwrap_or_default() {
                // 已是 OpenAI 形状。
                "function" => Some(choice.clone()),
                // Anthropic 指定工具 → OpenAI function 对象。
                "tool" => map
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| json!({ "type": "function", "function": { "name": name } })),
                // Anthropic 的 any / auto。
                "any" => Some(json!("required")),
                "auto" => Some(json!("auto")),
                _ => None,
            }
        }
        _ => None,
    }
}

/// CIR → Anthropic Messages 报文。
///
/// `max_tokens` 是必填项：客户端没给就用配置里该模型的输出上限，再兜底 4096。
pub(super) fn build_anthropic_body(request: &CanonicalRequest, configured_max_output: Option<u64>) -> Value {
    let mut body = match request.raw.clone() {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    if let Some(map) = body.as_object_mut() {
        map.retain(|key, _| !key.starts_with('_'));
        // 客户端是 OpenAI 协议时的字段，Anthropic 不认。
        map.remove("stream_options");
        map.remove("max_completion_tokens");
        map.remove("reasoning_effort");
    }

    let system: Vec<&str> = request.system.iter().filter_map(Block::as_text).collect();
    if system.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.remove("system");
        }
    } else {
        body["system"] = json!(system.join("\n"));
    }
    body["messages"] = Value::Array(anthropic_messages(request));
    body["model"] = json!(request.model);
    body["max_tokens"] = json!(request
        .max_tokens
        .or(configured_max_output)
        .unwrap_or(4096));
    body["stream"] = json!(true);

    if request.tools.is_empty() {
        if let Some(map) = body.as_object_mut() {
            map.remove("tools");
        }
    } else {
        body["tools"] = json!(request
            .tools
            .iter()
            .map(|tool| {
                let mut value = json!({ "name": tool.name });
                if let Some(description) = &tool.description {
                    value["description"] = json!(description);
                }
                value["input_schema"] = tool
                    .parameters
                    .clone()
                    .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                value
            })
            .collect::<Vec<_>>());
    }

    if let Some(choice) = &request.tool_choice {
        if let Some(normalized) = normalize_tool_choice_anthropic(choice) {
            body["tool_choice"] = normalized;
        }
    }
    body
}

/// CIR → Anthropic messages。**必须合并相邻同角色消息**：
/// Anthropic 要求 user/assistant 严格交替，客户端用 OpenAI 语义发来的
/// 连续 user 消息会被直接 400。
fn anthropic_messages(request: &CanonicalRequest) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        // tool 结果在 Anthropic 里是 user 消息里的 tool_result 块。
        let role = match message.role {
            Role::Assistant => "assistant",
            Role::User | Role::Tool => "user",
        };
        let blocks = anthropic_blocks(message);
        if blocks.is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some(last) if last["role"] == json!(role) => {
                if let Some(existing) = last["content"].as_array_mut() {
                    existing.extend(blocks);
                }
            }
            _ => messages.push(json!({ "role": role, "content": blocks })),
        }
    }
    messages
}

fn anthropic_blocks(message: &Message) -> Vec<Value> {
    let mut blocks = Vec::new();
    // tool 结果单独成块，没有正文。
    if let Some(id) = &message.tool_call_id {
        let text: Vec<&str> = message.content.iter().filter_map(Block::as_text).collect();
        blocks.push(json!({
            "type": "tool_result",
            "tool_use_id": id,
            "content": text.join("\n"),
        }));
        return blocks;
    }
    for block in &message.content {
        match block {
            Block::Text { text } => blocks.push(json!({ "type": "text", "text": text })),
            Block::Image { url } => blocks.push(json!({
                "type": "image",
                "source": { "type": "url", "url": url },
            })),
        }
    }
    for call in &message.tool_calls {
        let input: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
        blocks.push(json!({
            "type": "tool_use",
            "id": call.id.clone().unwrap_or_default(),
            "name": call.name,
            "input": input,
        }));
    }
    // 历史思考内容：Anthropic 只在开启 thinking 时才接受，这里不回传。
    blocks
}

pub(super) fn normalize_tool_choice_anthropic(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "required" => Some(json!({ "type": "any" })),
            "none" => Some(json!({ "type": "none" })),
            "auto" => Some(json!({ "type": "auto" })),
            _ => None,
        },
        Value::Object(map) => match map.get("type").and_then(Value::as_str).unwrap_or_default() {
            // 已是 Anthropic 形状。
            "auto" | "any" | "tool" | "none" => Some(choice.clone()),
            // OpenAI function 对象 → Anthropic 指定工具。
            "function" => map
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(|name| json!({ "type": "tool", "name": name })),
            _ => None,
        },
        _ => None,
    }
}

/// CIR → OpenAI Responses 报文（官方 `/responses` 形状）。
///
/// 与 Chat Completions 的硬差异（对照官方 OpenAPI spec 核实）：
/// · `input`（字符串或 item 数组）+ `instructions`，**没有** `messages` / `system`；
/// · 工具是**扁平** function（`{type:"function", name, ...}`，不套一层 `function`），
///   且官方 schema 把 `strict` 列为必填 —— 不带会被严格校验拒收；
/// · 上限字段是 `max_output_tokens`（不是 `max_tokens`）。
///
/// `input` 刻意用**消息 item 数组**而不是纯字符串：多轮对话与工具结果必须保序、
/// 保角色，压成字符串会把角色边界抹掉。
pub(super) fn build_responses_body(request: &CanonicalRequest) -> Value {
    // 只取采样参数等安全透传字段，不整包照抄：Responses 没有
    // `messages`/`stream_options`/`max_tokens` 这些字段，残留会被严格校验拒收。
    let mut body = serde_json::Map::new();
    if let Value::Object(map) = &request.raw {
        for (key, value) in map {
            if key.starts_with('_') {
                continue;
            }
            if matches!(
                key.as_str(),
                "model"
                    | "input"
                    | "instructions"
                    | "messages"
                    | "system"
                    | "tools"
                    | "tool_choice"
                    | "max_tokens"
                    | "max_completion_tokens"
                    | "max_output_tokens"
                    | "stream"
                    | "stream_options"
                    | "reasoning"
                    | "previous_response_id"
            ) {
                continue;
            }
            body.insert(key.clone(), value.clone());
        }
    }

    if !request.system.is_empty() {
        let system: Vec<&str> = request.system.iter().filter_map(Block::as_text).collect();
        body.insert("instructions".into(), json!(system.join("\n")));
    }

    body.insert("input".into(), Value::Array(responses_input(request)));
    body.insert("model".into(), json!(request.model));

    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            json!(request
                .tools
                .iter()
                .map(|tool| {
                    let mut value = json!({ "type": "function", "strict": false });
                    value["name"] = json!(tool.name);
                    if let Some(description) = &tool.description {
                        value["description"] = json!(description);
                    }
                    value["parameters"] = tool
                        .parameters
                        .clone()
                        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                    value
                })
                .collect::<Vec<_>>()),
        );
    }
    if let Some(choice) = &request.tool_choice {
        // Responses 的指定工具形状与 chat 不同：`{type:"function", name}`（不嵌套）。
        if let Some(normalized) = normalize_tool_choice_responses(choice) {
            body.insert("tool_choice".into(), normalized);
        }
    } else if !request.tools.is_empty() {
        // 显式落默认值：有的兼容实现缺这个字段会拒收。
        body.insert("tool_choice".into(), json!("auto"));
    }
    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_output_tokens".into(), json!(max_tokens));
    }
    // 思考：档位透传（官方 `reasoning.effort`，low/medium/high/max 是公共子集）。
    if let Some(reasoning) = &request.reasoning {
        if reasoning.enabled != Some(false) {
            let mut value = serde_json::Map::new();
            if let Some(effort) = &reasoning.effort {
                value.insert("effort".into(), json!(effort));
            }
            body.insert("reasoning".into(), Value::Object(value));
        }
    }
    body.insert("stream".into(), json!(true));
    // 网关无状态：明确关服务端存储，状态由客户端自己带（previous_response_id 已在
    // 入口侧拒绝，这里的 store=false 与那一策略一致）。
    body.insert("store".into(), json!(false));
    Value::Object(body)
}

/// CIR 消息流 → Responses input items。tool 结果是独立的 `function_call_output` item。
fn responses_input(request: &CanonicalRequest) -> Vec<Value> {
    let mut items = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::Tool => {
                let text: Vec<&str> = message.content.iter().filter_map(Block::as_text).collect();
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": message.tool_call_id.clone().unwrap_or_default(),
                    "output": text.join("\n"),
                }));
            }
            Role::Assistant => {
                if !message.tool_calls.is_empty() {
                    for call in &message.tool_calls {
                        items.push(json!({
                            "type": "function_call",
                            "call_id": call.id.clone().unwrap_or_default(),
                            "name": call.name,
                            "arguments": call.arguments,
                        }));
                    }
                }
                let text: Vec<&str> = message.content.iter().filter_map(Block::as_text).collect();
                if !text.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text.join("\n") }],
                    }));
                }
            }
            Role::User => {
                let text: Vec<&str> = message.content.iter().filter_map(Block::as_text).collect();
                let images: Vec<&str> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Image { url } => Some(url.as_str()),
                        Block::Text { .. } => None,
                    })
                    .collect();
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({ "type": "input_text", "text": text.join("\n") }));
                }
                for url in images {
                    content.push(json!({ "type": "input_image", "image_url": url }));
                }
                items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content,
                }));
            }
        }
    }
    items
}

/// tool_choice 归一到 Responses 形状（三字符串或 `{type:"function", name}`，不嵌套）。
fn normalize_tool_choice_responses(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(text) => {
            let lowered = text.trim().to_ascii_lowercase();
            matches!(lowered.as_str(), "auto" | "required" | "none").then(|| json!(lowered))
        }
        Value::Object(map) => {
            match map.get("type").and_then(Value::as_str).unwrap_or_default() {
                // 已是 Responses 形状。
                "function" if map.get("name").is_some() => Some(choice.clone()),
                // chat 的嵌套形状 → Responses 扁平形状。
                "function" => map
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .map(|name| json!({ "type": "function", "name": name })),
                // Anthropic 指定工具。
                "tool" => map
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| json!({ "type": "function", "name": name })),
                "any" => Some(json!("required")),
                "auto" => Some(json!("auto")),
                _ => None,
            }
        }
        _ => None,
    }
}
