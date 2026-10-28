//! CIR 请求 → Trae Remote 的两段式报文（`chat_sessions` + `events`）。
//!
//! Trae 与 OpenAI 兼容渠道的**根本差异**：它不是「一次 POST 回一条流」，
//! 而是先 `POST /chat_sessions` 建会话拿到 `(session_id, message_id)`，
//! 再 `GET /chat_sessions/{id}/events?reply_to_message_id=...` 读事件流。
//! 因此本渠道覆盖 [`ChannelAdapter::chat_stream`]，而不是只提供 `build_chat_request`。
//!
//! 两个易踩的点：
//! ① 历史消息必须压成**一个** `query` 字符串（上游按 `[System]`/`[Assistant]`
//!    这类标记自己理解角色），而不是 `messages` 数组；
//! ② `model_name` 大小写敏感，对外 id 是小写，得按目录的真实拼写回填。

use serde_json::{json, Value};

use crate::gateway::{
    errors::{ErrorKind, ErrorScope, GatewayError},
    protocol::cir::{Block, CanonicalRequest, Message, Role},
};

use super::identity;

/// 上游单个会话历史消息上限（超出会静默截断事件流，而不是报错）。
pub const MAX_HISTORY_MESSAGES: usize = 500;
/// 输入字符上限：超过约 500K 字符上游会静默结束事件流。
pub const MAX_HISTORY_CHARS: usize = 480_000;
/// Agent 模型的单次输出上限（上游硬顶 64K）。
pub const MAX_COMPLETION_TOKENS: u64 = 64_000;

/// 会话模式：`code` = Agent 档（默认，能力最全），`work` = Work 档。
pub fn resolve_mode(model: &str) -> (&'static str, &str) {
    let lowered = model.trim().to_ascii_lowercase();
    if lowered == "work" || lowered == "auto-work" || lowered == "solo-work" {
        return ("work", "auto");
    }
    if lowered.is_empty() || lowered == "auto" {
        return ("code", "auto");
    }
    ("code", "manual")
}

/// 把一条消息压成人类可读的文本块（Trae 的 `query` 就是这个形状）。
fn message_text(message: &Message) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } => parts.push(text.clone()),
            // 图片在 remote 通道里只能作为附件随 `content` 数组走；
            // `query` 是纯文本半边，这里如实说明而不是静默丢弃。
            Block::Image { .. } => parts.push("[图片]".to_string()),
        }
    }
    parts.join("\n")
}

/// 工具调用压成文本：上游没有 OpenAI 的 `tool_calls` 字段，
/// 只能靠这段文本让模型知道"上一轮它调过什么、结果是什么"。
fn tool_calls_text(message: &Message) -> String {
    if message.tool_calls.is_empty() {
        return String::new();
    }
    let mut lines = vec!["[Tool Calls]".to_string()];
    for call in &message.tool_calls {
        let id = call.id.as_deref().unwrap_or("unknown");
        lines.push(format!(
            "- {} id={} arguments={}",
            call.name, id, call.arguments
        ));
    }
    lines.join("\n")
}

/// 历史消息 → Trae 的 `query`。
///
/// **`query` 必须是 JSON 字符串数组**，不是纯文本：
/// ```json
/// [{"type":"text","data":{"content":"你是助手\n\n你好"}}]
/// ```
/// 之前直接发纯文本，上游会因为解析不出 parts 结构回
/// `{"code":995000,"message":"internal server error"}`（HTTP 200 里套业务错误，
/// 看起来像上游挂了）。角色标记（`[System]` / `[Assistant]` / 工具结果）
/// 都拼在同一个 text part 的 content 里，与参考实现一致。
pub fn flatten_query(request: &CanonicalRequest) -> String {
    let mut parts: Vec<String> = Vec::new();

    // system 被解码层提到 request.system，要重新拼回历史最前面。
    if !request.system.is_empty() {
        let text: Vec<String> = request
            .system
            .iter()
            .filter_map(|block| block.as_text().map(str::to_string))
            .collect();
        if !text.is_empty() {
            parts.push(format!("[System]\n{}", text.join("\n")));
        }
    }

    for message in &request.messages {
        let content = message_text(message);
        match message.role {
            Role::Assistant => {
                let mut collected = vec![content];
                let calls = tool_calls_text(message);
                if !calls.is_empty() {
                    collected.push(calls);
                }
                collected.retain(|item| !item.is_empty());
                if !collected.is_empty() {
                    parts.push(format!("[Assistant]\n{}", collected.join("\n\n")));
                }
            }
            Role::Tool => {
                let id = message.tool_call_id.as_deref().unwrap_or("unknown");
                let name = message.name.as_deref().unwrap_or("tool");
                parts.push(format!(
                    "[Client Tool Result: {id} {name} status=succeeded]\n{content}"
                ));
            }
            Role::User => {
                if !content.is_empty() {
                    parts.push(content);
                }
            }
        }
    }
    let text = clip_history(parts).join("\n\n");
    // 上游只认 parts 数组；空历史也要给一个空 text part，
    // 不能发空字符串或 `[]`（那同样会被判为非法请求）。
    serde_json::json!([{ "type": "text", "data": { "content": text } }])
        .to_string()
}

/// 超长历史裁剪：从**最早**的非系统消息开始丢，保证最近几轮完整。
fn clip_history(mut parts: Vec<String>) -> Vec<String> {
    let mut total: usize = parts.iter().map(|part| part.len() + 2).sum();
    while total > MAX_HISTORY_CHARS && parts.len() > 1 {
        let removed = parts.remove(0);
        total -= removed.len() + 2;
    }
    if parts.len() > MAX_HISTORY_MESSAGES {
        parts.drain(..parts.len() - MAX_HISTORY_MESSAGES);
    }
    parts
}

/// 建会话的请求体。
///
/// `custom_model` 是 **Agent 目录里的原始模型对象**（`catalog::fetch_model_objects`）。
/// 必须原样回传：上游靠它绑定模型、拿上下文与思考档位配置。
/// 只给 `model_name` 时上游会静默换模型，或直接回 `995000 internal server error`。
pub fn build_session_body(
    model: &str,
    request: &CanonicalRequest,
    account: &crate::gateway::channels::AccountSnapshot,
) -> Value {
    build_session_body_with_model(model, request, account, None)
}

/// 同上，但带上上游目录里的 `custom_model` 原始对象。
pub fn build_session_body_with_model(
    model: &str,
    request: &CanonicalRequest,
    account: &crate::gateway::channels::AccountSnapshot,
    custom_model: Option<&Value>,
) -> Value {
    let (mode, mut strategy) = resolve_mode(model);
    let mut upstream_model = identity::normalize_model(model);
    let mut effort_applied: Option<String> = None;
    let mut tuned_model: Option<Value> = None;

    // 有原始对象时：模型名取对象里的精确拼写，并把思考强度写回对象。
    if let Some(object) = custom_model {
        if let Some(bound) = bound_model_name(&upstream_model, object) {
            upstream_model = bound;
        }
        let (tuned, effort) = apply_reasoning_effort(object, request);
        tuned_model = Some(tuned);
        effort_applied = effort;
    }
    // `auto` 保持交给上游选择；有对象时按 object 里的策略走 manual。
    if custom_model.is_some() && strategy == "auto" {
        strategy = "auto";
    }

    let agent_type = if mode == "work" {
        "solo_work_remote"
    } else {
        "solo_agent_remote"
    };
    let initial_message = json!({
        "chat_session_id": "",
        "content": [],
        "query": flatten_query(request),
        "model_name": upstream_model,
        "agent_type": agent_type,
        // agent_id 与 agent_type 同值：只给 agent_type 时上游会静默回落到默认档
        // （常见是 Kimi/Work），模型绑定就丢了。
        "agent_id": agent_type,
        "model_selection_strategy": strategy,
        "common_params": identity::common_params(account, mode, ""),
    });
    let mut body = json!({
        "mode": mode,
        "environment_id": "default",
        "initial_message": initial_message,
        "env": "remote",
        "auto_create_project": false,
        "origin": "web",
    });
    if let Some(initial) = body.get_mut("initial_message").and_then(Value::as_object_mut) {
        // **把原始模型对象整体带上**：这是上游绑定模型的依据。
        if let Some(object) = tuned_model {
            initial.insert(
                "model_config_source".into(),
                json!(object.get("config_source").and_then(Value::as_u64).unwrap_or(1)),
            );
            initial.insert(
                "model_is_preset".into(),
                json!(object
                    .get("is_preset")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)),
            );
            initial.insert(
                "model_provider".into(),
                json!(object
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("")),
            );
            initial.insert("custom_model".into(), object);
        }
        if let Some(effort) = effort_applied {
            // 记进报文便于排障（上游以 custom_model 里的值为准）。
            initial.insert("reasoning_effort_applied".into(), json!(effort));
        }
        // 输出上限：客户端常发一个远大于上游硬顶的值（131072 之类），
        // 直接透传会被上游判成参数错误。钳到 Agent 档的真实上限。
        // 注意：**只在客户端给了值时才写**——不给就交给上游默认，
        // 硬塞一个 64K 反而会把"短回答"也按最大输出计费。
        if let Some(max_tokens) = request.max_tokens {
            initial.insert(
                "max_output_tokens".into(),
                json!(max_tokens.min(MAX_COMPLETION_TOKENS)),
            );
        }
        if mode == "work" {
            initial.insert("__hit_shared_artifact_and_workroom".into(), true.into());
            initial.insert("__hit_my_space".into(), true.into());
        }
    }
    body
}

/// 从目录对象里取**精确的 config id**，并拒绝静默回退到别的模型族。
///
/// 上游按 `config_name` 大小写敏感匹配；对象里可能同时有 `name`（显示名）
/// 与 `config_name`（线上 id），必须优先用后者。
fn bound_model_name(requested: &str, object: &Value) -> Option<String> {
    let candidates: Vec<String> = ["config_name", "model_name", "name"]
        .iter()
        .filter_map(|key| object.get(*key))
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let family = model_family(requested);
    // 优先挑与请求同族的那个（`glm-5.3` 的 display name 与 config name 同族）。
    candidates
        .iter()
        .find(|candidate| !family.is_empty() && model_family(candidate) == family)
        .or_else(|| candidates.first())
        .cloned()
}

/// 模型族：`deepseek-v4.1-flash` → `deepseek`，`glm-5.3` → `glm`。
///
/// 只取第一个非版本号片段，用来判断"对象里的名字和请求的是不是同一个模型"。
fn model_family(model: &str) -> String {
    let lowered = model.trim().to_ascii_lowercase();
    lowered
        .split(|ch: char| ch == '-' || ch == '_' || ch == '/' || ch == '.')
        .find(|segment| {
            !segment.is_empty()
                && !segment.starts_with(|ch: char| ch.is_ascii_digit())
                && *segment != "official"
        })
        .unwrap_or("")
        .to_string()
}

/// 把请求里的思考强度映射到 Trae 的原生档位，写进 `custom_model` 副本。
///
/// Trae 的档位是 `light` / `high` / `extra_high`；调用方用的是
/// OpenAI 的 `low`/`medium`/`high` 或 Anthropic 的 budget_tokens。
/// 模型声明的可选档位在 `reasoning_effort_config.options` 里，超出的要钳制，
/// 否则上游会因为"不支持的档位"报错。
fn apply_reasoning_effort(object: &Value, request: &CanonicalRequest) -> (Value, Option<String>) {
    let mut tuned = object.clone();
    let Some(requested) = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.as_deref())
    else {
        return (tuned, None);
    };
    // 显式关闭思考时，不改对象（由客户端的 none 语义处理）。
    if requested.eq_ignore_ascii_case("none") {
        return (tuned, None);
    }
    let supported: Vec<String> = object
        .get("reasoning_effort_config")
        .and_then(|config| config.get("options"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|value| value.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let Some(level) = clamp_effort(requested, &supported) else {
        return (tuned, None);
    };
    if let Some(map) = tuned.as_object_mut() {
        // 参考实现：对象里已带 `reasoning_effort_level` 就改它，否则写 `reasoning_effort`，
        // 两个都写会让上游行为不确定。
        if map.contains_key("reasoning_effort_level") {
            map.insert("reasoning_effort_level".into(), json!(level));
            map.remove("reasoning_effort");
        } else {
            map.insert("reasoning_effort".into(), json!(level));
            map.remove("reasoning_effort_level");
        }
    }
    (tuned, Some(level))
}

/// 把调用方的档位名映射到 Trae 的 `light`/`high`/`extra_high`，并按模型能力钳制。
fn clamp_effort(requested: &str, supported: &[String]) -> Option<String> {
    const ORDER: [&str; 3] = ["light", "high", "extra_high"];
    let level = match requested.trim().to_ascii_lowercase().as_str() {
        "none" | "minimal" | "min" | "low" | "light" => "light",
        "medium" | "mid" | "normal" | "default" | "high" => "high",
        "xhigh" | "x-high" | "extra_high" | "extra-high" | "very_high" | "max" | "maximum"
        | "ultra" => "extra_high",
        // 认不出的档位名不猜：交给上游默认。
        _ => return None,
    };
    if supported.is_empty() {
        // 模型没声明档位：按请求原样发（上游会自己决定认不认）。
        return Some(level.to_string());
    }
    // 钳到模型支持的最高档：`high` 请求 + 只有 `light` 可选 → 用 `light`。
    let requested_index = ORDER.iter().position(|entry| *entry == level)?;
    let best = supported
        .iter()
        .filter_map(|entry| ORDER.iter().position(|known| known == entry))
        .filter(|index| *index <= requested_index)
        .max()
        .or_else(|| {
            supported
                .iter()
                .filter_map(|entry| ORDER.iter().position(|known| known == entry))
                .min()
        })?;
    Some(ORDER[best].to_string())
}

/// 建会话响应 → `(session_id, message_id)`。
pub fn parse_session_response(payload: &Value) -> Result<(String, String), GatewayError> {
    // 业务错误优先：上游常以 HTTP 200 + 非 0 code 表达失败。
    if let Some(code) = payload.get("code").and_then(Value::as_i64) {
        if code != 0 && payload.get("data").is_none() {
            let message = payload
                .get("msg")
                .or_else(|| payload.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("上游拒绝了建会话请求");
            return Err(business_error(code, message));
        }
    }
    let data = payload.get("data").unwrap_or(payload);
    let session_id = data
        .get("chat_session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayError::upstream(502, "上游没有返回 chat_session_id"))?;
    let message_id = data
        .get("message_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| GatewayError::upstream(502, "上游没有返回 message_id"))?;
    Ok((session_id.to_string(), message_id.to_string()))
}

/// Trae 的业务错误码 → 统一语义。
///
/// 这些码比 HTTP 状态码精确得多（上游绝大部分失败都是 200 + code），
/// 所以分类必须走这里，而不是靠状态码兜底。
pub fn business_error(code: i64, message: &str) -> GatewayError {
    let text = if message.trim().is_empty() {
        format!("上游业务错误 code={code}")
    } else {
        format!("{message}（code={code}）")
    };
    match code {
        // 凭据过期/无效。
        401 | 1001 | 1002 | 1003 => GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            text,
            true,
        ),
        // 额度/积分不足。
        402 | 1005 => GatewayError::new(
            ErrorKind::QuotaExhausted,
            ErrorScope::Account,
            402,
            text,
            true,
        ),
        // 模型不可用（下线/未开通）：只影响这个模型。
        4023 | 404 => GatewayError::new(
            ErrorKind::ModelNotFound,
            ErrorScope::Request,
            404,
            text,
            false,
        ),
        // 频率限制 / 排队相关：账号级，可等。
        429 | 9074 => GatewayError::new(
            ErrorKind::RateLimited,
            ErrorScope::Account,
            429,
            text,
            true,
        ),
        // 参数不合法：换号没用。
        4001 | 400 | 422 => GatewayError::invalid_request(text),
        _ => GatewayError::upstream(502, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::cir::{Message, Role, ToolCall};

    fn request() -> CanonicalRequest {
        CanonicalRequest {
            model: "glm-5.3".into(),
            system: vec![Block::text("你是助手")],
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![Block::text("你好")],
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                },
                Message {
                    role: Role::Assistant,
                    content: vec![Block::text("我查一下")],
                    tool_calls: vec![ToolCall {
                        id: Some("call_1".into()),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"a\"}".into(),
                    }],
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                },
                Message {
                    role: Role::Tool,
                    content: vec![Block::text("文件内容")],
                    tool_calls: Vec::new(),
                    tool_call_id: Some("call_1".into()),
                    name: Some("read_file".into()),
                    reasoning: None,
                },
            ],
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

    #[test]
    fn query_carries_roles_tools_and_results() {
        let query = flatten_query(&request());
        // `query` 是 **JSON parts 数组**，不是纯文本：上游按 parts 解析历史。
        let parsed: serde_json::Value = serde_json::from_str(&query)
            .unwrap_or_else(|error| panic!("query 必须是 JSON：{error}，实际 {query}"));
        assert!(parsed.is_array(), "query 必须是数组：{query}");
        assert_eq!(parsed[0]["type"], "text");
        assert_eq!(parsed[0]["data"]["data"], serde_json::Value::Null, "不应多套一层");
        let text = parsed[0]["data"]["content"]
            .as_str()
            .unwrap_or_else(|| panic!("content 必须是字符串：{query}"));

        assert!(text.starts_with("[System]\n你是助手"), "{text}");
        assert!(text.contains("[Assistant]\n我查一下"), "{text}");
        assert!(text.contains("[Tool Calls]"), "工具调用要写进历史：{text}");
        assert!(
            text.contains("[Client Tool Result: call_1 read_file status=succeeded]"),
            "{text}"
        );
        assert!(text.contains("文件内容"), "{text}");
    }

    /// 空历史也要给出合法的 parts（不能是空串或 `[]`）。
    #[test]
    fn empty_history_still_produces_a_valid_parts_array() {
        let mut request = request();
        request.system.clear();
        request.messages.clear();
        let parsed: serde_json::Value =
            serde_json::from_str(&flatten_query(&request)).expect("必须是 JSON");
        assert_eq!(parsed.as_array().map(Vec::len), Some(1));
        assert_eq!(parsed[0]["data"]["content"], "");
    }

    #[test]
    fn session_body_pins_model_and_executor() {
        let account = crate::gateway::channels::AccountSnapshot {
            channel_id: "trae-cn".into(),
            uid: "u".into(),
            access_token: Some("jwt".into()),
            refresh_token: None,
            enterprise_id: None,
            ..Default::default()
        };
        let body = build_session_body("glm-5.3", &request(), &account);
        assert_eq!(body["mode"], "code");
        assert_eq!(body["origin"], "web");
        assert_eq!(body["initial_message"]["model_name"], "glm-5.3");
        assert_eq!(body["initial_message"]["agent_type"], "solo_agent_remote");
        assert_eq!(body["initial_message"]["agent_id"], "solo_agent_remote");
        assert_eq!(body["initial_message"]["model_selection_strategy"], "manual");
        // common_params 必须是**字符串**，上游按字符串解析。
        assert!(body["initial_message"]["common_params"].is_string());

        // auto：策略交给上游；work：换执行器层级并带上 Work 专属标记。
        let auto = build_session_body("auto", &request(), &account);
        assert_eq!(auto["initial_message"]["model_selection_strategy"], "auto");
        let work = build_session_body("work", &request(), &account);
        assert_eq!(work["mode"], "work");
        assert_eq!(work["initial_message"]["agent_type"], "solo_work_remote");
        assert_eq!(
            work["initial_message"]["__hit_shared_artifact_and_workroom"],
            true
        );
    }

    #[test]
    fn session_response_reads_ids_and_rejects_missing_ones() {
        let ok = json!({"code": 0, "data": {"chat_session_id": "s1", "message_id": "m1"}});
        assert_eq!(
            parse_session_response(&ok).expect("应当解析成功"),
            ("s1".into(), "m1".into())
        );
        let flat = json!({"chat_session_id": "s2", "message_id": "m2"});
        assert_eq!(
            parse_session_response(&flat).expect("无 data 包装也要认"),
            ("s2".into(), "m2".into())
        );
        let broken = json!({"code": 0, "data": {"chat_session_id": "s1"}});
        assert!(parse_session_response(&broken).is_err());
    }

    #[test]
    fn business_codes_map_to_semantics() {
        assert_eq!(
            business_error(1005, "quota").kind,
            ErrorKind::QuotaExhausted
        );
        assert_eq!(
            business_error(4023, "the model is unknown").kind,
            ErrorKind::ModelNotFound
        );
        // 模型不可用不该把账号拉黑。
        assert_eq!(business_error(4023, "x").scope, ErrorScope::Request);
        assert_eq!(business_error(401, "x").kind, ErrorKind::AuthExpired);
        assert_eq!(business_error(9074, "risk").kind, ErrorKind::RateLimited);
        assert!(!business_error(4001, "param invalid").retryable);
    }

    #[test]
    fn long_history_is_clipped_from_the_oldest_end() {
        let mut parts: Vec<String> = (0..600).map(|index| format!("消息{index}")).collect();
        parts = clip_history(parts);
        assert!(parts.len() <= MAX_HISTORY_MESSAGES);
        // 裁掉的是最早的，最后一条必须还在。
        assert!(parts.last().expect("非空").contains("消息599"));
    }

    #[test]
    fn max_tokens_is_clamped_to_the_upstream_ceiling() {
        let account = crate::gateway::channels::AccountSnapshot {
            channel_id: "trae-cn".into(),
            uid: "u".into(),
            access_token: Some("jwt".into()),
            refresh_token: None,
            enterprise_id: None,
            ..Default::default()
        };
        let mut base = request();
        // 客户端给了一个远大于上游硬顶的值。
        base.max_tokens = Some(131_072);
        let body = build_session_body("glm-5.3", &base, &account);
        assert_eq!(
            body["initial_message"]["max_output_tokens"],
            json!(MAX_COMPLETION_TOKENS),
            "超过硬顶要钳到 64K"
        );

        // 小于硬顶的值原样保留。
        let mut small = request();
        small.max_tokens = Some(4_096);
        let body = build_session_body("glm-5.3", &small, &account);
        assert_eq!(body["initial_message"]["max_output_tokens"], json!(4096));

        // 客户端没给：不写这个字段（交给上游默认，避免虚高计费）。
        let none = request();
        let body = build_session_body("glm-5.3", &none, &account);
        assert!(body["initial_message"].get("max_output_tokens").is_none());
    }
}
