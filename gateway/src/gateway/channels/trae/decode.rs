//! Trae Remote 事件流 → CIR 事件。
//!
//! 上游帧形状与 OpenAI **完全不同**：没有 `choices`/`delta`，而是
//! `event:` + `data:` 两段式，正文以 **累计快照** 的形式反复下发
//! （`response` 字段里是到目前为止的全文，不是增量）。
//!
//! 所以解码必须做成**有状态**的：记住上一帧的全文长度，只吐新增部分。
//! 把累计值当增量直接透传，客户端会看到"你好你好你好…"这样重复膨胀的正文。

use std::sync::Mutex;

use serde_json::Value;

use crate::gateway::protocol::cir::{CanonicalEvent, Usage};

/// 单个流内的解码状态。每帧都从同一份状态里取上一次的长度。
#[derive(Default)]
pub struct StreamState {
    text: String,
    reasoning: String,
    /// 已经吐出去的用量（上游会重复下发同一份 usage）。
    finished: bool,
}

/// 有状态的解码器：由渠道实例为每次请求新建一个。
pub struct Decoder {
    state: Mutex<StreamState>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            state: Mutex::new(StreamState::default()),
        }
    }
}

impl Decoder {
    /// 一帧（可能含 `event:` 行与多行 `data:`）→ 若干 CIR 事件。
    ///
    /// 未知帧返回空 vec：多一个字段不该中断整条流。
    pub fn decode(&self, frame: &str) -> Vec<CanonicalEvent> {
        let (event_name, payload) = split_frame(frame);
        let Ok(value) = serde_json::from_str::<Value>(&payload) else {
            return Vec::new();
        };
        let name = normalize_event_name(&event_name, &value);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        decode_json(&name, &value, &mut state)
    }

    /// 流结束时补齐 `Finish`：上游常在最后一帧才给 `done`，
    /// 但断流/超时的会话不会给 —— 由调用方在帧流耗尽后调用一次。
    pub fn finish(&self) -> Vec<CanonicalEvent> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.finished {
            return Vec::new();
        }
        state.finished = true;
        vec![CanonicalEvent::Finish {
            reason: "stop".to_string(),
        }]
    }
}

/// 拆一帧：`event:` 名 + `data:` 负载（多行用换行拼接）。
fn split_frame(frame: &str) -> (String, String) {
    let mut event = String::new();
    let mut data: Vec<String> = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
    }
    (event, data.join("\n"))
}

/// 事件名归一：上游可能给 camelCase / snake_case / PascalCase 三种拼法。
pub fn normalize_event_name(raw: &str, value: &Value) -> String {
    let candidate = if raw.trim().is_empty() {
        ["event", "event_type", "eventType", "type"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .unwrap_or("")
            .to_string()
    } else {
        raw.trim().to_string()
    };
    let mut out = String::with_capacity(candidate.len() + 4);
    for (index, ch) in candidate.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let collapsed = out.trim_matches('_').to_string();
    match collapsed.replace('_', "").as_str() {
        "keepalive" => "keepalive".into(),
        "modelconfig" => "model_config".into(),
        "planitem" => "plan_item".into(),
        "requestwaitinqueue" => "request_wait_in_queue".into(),
        "responsedone" | "streamdone" => "done".into(),
        "tokenusage" => "token_usage".into(),
        _ => collapsed,
    }
}

fn decode_json(name: &str, value: &Value, state: &mut StreamState) -> Vec<CanonicalEvent> {
    let mut events = Vec::new();

    if name == "error" {
        let message = value
            .get("message")
            .or_else(|| value.get("msg"))
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("Trae 上游返回 error 事件");
        events.push(CanonicalEvent::Error {
            message: message.to_string(),
        });
        return events;
    }

    // 排队提示：如实透出成正文（与参考实现一致），而不是让客户端干等。
    if name == "request_wait_in_queue" {
        if let Some(position) = value.get("position").and_then(Value::as_u64) {
            events.push(CanonicalEvent::Text {
                text: format!("排队中，当前位置：{position}\n"),
            });
        }
        return events;
    }

    if name == "token_usage" {
        let usage = value.get("usage").unwrap_or(value);
        if let Some(usage) = parse_usage(usage) {
            events.push(CanonicalEvent::Usage(usage));
        }
        return events;
    }

    // **`plan_item` 才是正文的真实载体**。
    //
    // SOLO Agent 的回复不走 `content` delta，而是以「计划项」形式下发：
    // ```json
    // {"id":"pi_...","thought":"你好","reasoning_content":"",
    //  "tool_call_info":{"name":"","params":null}}
    // ```
    // `thought` 是**累计快照**（同一个 `id` 会重复下发多次），最后的
    // `tool_call_info.name = "finish"` 里 `params.summary` 是最终答案。
    // 之前完全不认这个事件，所以"建会话成功但正文为空"。
    if name == "plan_item" {
        let visible = plan_item_text(value);
        if let Some(delta) = push_delta(&mut state.text, &visible) {
            events.push(CanonicalEvent::Text { text: delta });
        }
        // 工具类 plan item 才转成工具调用；`finish` 是终止标记，不是调用。
        if let Some(call) = plan_item_tool_call(value) {
            events.push(call);
        }
        return events;
    }

    // 思考内容：与正文分开，绝不混进 content。
    // 思考**两种形态都存在**（累计快照 / 逐段增量），所以按"能当增量就当增量"处理。
    let reasoning = extract_reasoning(value);
    if let Some(delta) = push_delta_lenient(&mut state.reasoning, &reasoning) {
        events.push(CanonicalEvent::Reasoning { text: delta });
    }

    // 正文：`response` 是累计快照；`text_delta`/`translated_text` 才是真增量。
    let (text, cumulative) = extract_text(value, name);
    let delta = if cumulative {
        push_delta(&mut state.text, &text)
    } else {
        push_delta_lenient(&mut state.text, &text)
    };
    if let Some(delta) = delta {
        events.push(CanonicalEvent::Text { text: delta });
    }

    // 工具调用：上游形状是 `tool_call_info`，没有 OpenAI 的 tool_calls 数组。
    if let Some(call) = extract_tool_call(value) {
        events.push(call);
    }

    if name == "done" {
        if let Some(usage) = value.get("usage").and_then(parse_usage) {
            events.push(CanonicalEvent::Usage(usage));
        }
        state.finished = true;
        events.push(CanonicalEvent::Finish {
            reason: finish_reason(value),
        });
    }
    events
}

/// 累计值 → 增量：只有变长了才吐新增的那一段。
///
/// 上游偶尔会下发**更短**的快照（重试/回滚），这时不回退也不吐空，
/// 保持已发出的内容不变 —— 客户端看到的内容只增不减才不会闪烁。
pub fn push_delta(accumulated: &mut String, next: &str) -> Option<String> {
    if next.is_empty() {
        return None;
    }
    if !accumulated.is_empty() && next.starts_with(accumulated.as_str()) {
        let delta = next[accumulated.len()..].to_string();
        *accumulated = next.to_string();
        return (!delta.is_empty()).then_some(delta);
    }
    if accumulated.is_empty() {
        *accumulated = next.to_string();
        return Some(next.to_string());
    }
    if next.len() > accumulated.len() {
        // 不是前缀关系但变长了（上游改了开头）：只吐尾部新增，避免重复。
        let delta = next[accumulated.len()..].to_string();
        *accumulated = next.to_string();
        return (!delta.is_empty()).then_some(delta);
    }
    None
}

/// 增量语义的拼接：直接追加，不做前缀判定。
///
/// 用于 `text_delta` / `translated_text` 与思考增量 —— 这两类上游给的
/// **就是**本次新增的那一段。套用累计逻辑（`push_delta`）时，第二段
/// "（" 不以第一段 "再想" 开头、也不更长，会被当成"回滚"整段丢掉。
pub fn push_delta_lenient(accumulated: &mut String, delta: &str) -> Option<String> {
    if delta.is_empty() {
        return None;
    }
    accumulated.push_str(delta);
    Some(delta.to_string())
}

/// 取正文与「这段是累计值还是增量」。
///
/// `response` 是累计快照（要按前缀差量）；`text_delta` 一类是**真增量**
/// （直接追加）。两者搞反的表现：正文要么重复膨胀，要么整段丢失。
fn extract_text(value: &Value, name: &str) -> (String, bool) {
    // 累计快照优先（这个字段的语义是固定的）。
    if let Some(text) = value.get("response").and_then(Value::as_str) {
        return (text.to_string(), true);
    }
    // 真增量事件。
    let delta_events = [
        "text_delta",
        "translated_text",
        "accumulated",
        "assistant_message",
    ];
    if delta_events.contains(&name) {
        if let Some(text) = pick_text(value) {
            return (text, false);
        }
    }
    // remote 的 message 形状：content 数组里挑非 reasoning 的块。
    // message 事件是**快照**（整条 assistant 消息），按累计处理。
    for key in ["message", "agent_message", "assistant_message"] {
        if let Some(nested) = value.get(key) {
            if let Some(text) = pick_content_text(nested) {
                return (text, true);
            }
        }
    }
    (pick_content_text(value).unwrap_or_default(), true)
}

fn pick_text(value: &Value) -> Option<String> {
    for key in ["delta", "text_delta", "textDelta", "text", "content", "response"] {
        match value.get(key) {
            Some(Value::String(text)) => return Some(text.clone()),
            Some(Value::Object(_)) | Some(Value::Array(_)) => {
                if let Some(nested) = pick_content_text(value.get(key).expect("checked")) {
                    return Some(nested);
                }
            }
            _ => {}
        }
    }
    None
}

fn pick_content_text(value: &Value) -> Option<String> {
    match value.get("content") {
        Some(Value::String(text)) => return Some(text.clone()),
        Some(Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block {
                    Value::String(text) => parts.push(text.clone()),
                    Value::Object(map) => {
                        let kind = map
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        // 思考块不走正文。
                        if matches!(
                            kind.as_str(),
                            "reasoning" | "thinking" | "reasoning_text" | "thought"
                        ) {
                            continue;
                        }
                        if let Some(text) = map
                            .get("text")
                            .or_else(|| map.get("content"))
                            .and_then(Value::as_str)
                        {
                            parts.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if !parts.is_empty() {
                return Some(parts.join(""));
            }
        }
        _ => {}
    }
    for key in ["text", "response", "answer", "output"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    None
}

fn extract_reasoning(value: &Value) -> String {
    for key in ["reasoning_content", "reasoningContent", "thought", "thinking"] {
        match value.get(key) {
            Some(Value::String(text)) if !text.is_empty() => return text.clone(),
            Some(Value::Object(map)) => {
                for nested in ["content", "text", "summary"] {
                    if let Some(text) = map.get(nested).and_then(Value::as_str) {
                        if !text.is_empty() {
                            return text.to_string();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(message) = value.get("message") {
        let nested = extract_reasoning(message);
        if !nested.is_empty() {
            return nested;
        }
        if let Some(Value::Array(blocks)) = message.get("content") {
            let mut parts = Vec::new();
            for block in blocks {
                let Some(map) = block.as_object() else { continue };
                let kind = map
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(
                    kind.as_str(),
                    "reasoning" | "thinking" | "reasoning_text" | "thought"
                ) {
                    continue;
                }
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
            if !parts.is_empty() {
                return parts.join("");
            }
        }
    }
    String::new()
}

/// `plan_item` 的可见文本（累计快照）。
///
/// 上游把"思考轨迹"放 `thought`/`reasoning_content`，把"实际回复"放 `content`。
/// 两者都是累计快照，所以拼起来作为正文；只取一个会漏掉另一半。
/// 已经包含关系时不重复拼接（`content in thought` 很常见）。
fn plan_item_text(value: &Value) -> String {
    let thought = value
        .get("thought")
        .or_else(|| value.get("reasoning_content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let content = pick_content_text(value).unwrap_or_default();
    let content = content.trim();
    if content.is_empty() {
        return thought;
    }
    if thought.is_empty() {
        return content.to_string();
    }
    if thought.contains(content) {
        return thought;
    }
    format!("{thought}\n\n{content}")
}

/// `plan_item` 里的工具调用。
///
/// 只有**真的有工具名**的才转成调用；`finish` 是上游的终止标记，不是调用 ——
/// 把它当工具调用会让客户端去执行一个叫 "finish" 的函数。
fn plan_item_tool_call(value: &Value) -> Option<CanonicalEvent> {
    let info = value.get("tool_call_info")?;
    let name = info
        .get("name")
        .or_else(|| info.get("tool_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("finish"))?;
    let id = info
        .get("id")
        .or_else(|| info.get("tool_call_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let arguments = info
        .get("params")
        .or_else(|| info.get("arguments"))
        .map(|params| match params {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    Some(CanonicalEvent::ToolCall {
        index: 0,
        id,
        name: Some(name.to_string()),
        arguments_delta: (!arguments.is_empty()).then_some(arguments),
    })
}

/// `tool_call_info` 形状的工具调用。
fn extract_tool_call(value: &Value) -> Option<CanonicalEvent> {
    let info = value.get("tool_call_info")?;
    let name = info
        .get("name")
        .or_else(|| info.get("tool_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        // `finish` 是上游的终止标记（`params.summary` 才是最终答案），
        // 它不是客户端要执行的函数，不能当 tool_call 下发。
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("finish"))?
        .to_string();
    let id = info
        .get("tool_call_id")
        .or_else(|| info.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let arguments = info
        .get("arguments")
        .or_else(|| info.get("params"))
        .map(|params| match params {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    Some(CanonicalEvent::ToolCall {
        index: 0,
        id,
        name: Some(name),
        arguments_delta: (!arguments.is_empty()).then_some(arguments),
    })
}

fn parse_usage(value: &Value) -> Option<Usage> {
    if value.is_null() {
        return None;
    }
    let number = |key: &str| -> u64 {
        match value.get(key) {
            Some(Value::Number(number)) => number.as_u64().unwrap_or(0),
            Some(Value::String(text)) => text.parse().unwrap_or(0),
            _ => 0,
        }
    };
    let prompt = number("prompt_tokens").max(number("input_tokens"));
    let completion = number("completion_tokens").max(number("output_tokens"));
    let cached = number("cached_tokens").max(number("cache_read_input_tokens"));
    let reasoning = number("reasoning_tokens");
    let total = number("total_tokens").max(prompt + completion);
    if prompt == 0 && completion == 0 && total == 0 {
        return None;
    }
    Some(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        reasoning_tokens: reasoning,
        cached_tokens: cached,
        total_tokens: total,
        credit: value.get("credit").and_then(Value::as_f64),
    })
}

fn finish_reason(value: &Value) -> String {
    let raw = value
        .get("finish_reason")
        .or_else(|| value.get("stop_reason"))
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_ascii_lowercase();
    match raw.as_str() {
        "tool_calls" | "tool-calls" | "function_call" | "tool_use" | "tool" => "tool_calls".into(),
        "content_filter" | "content-filter" | "safety" => "content_filter".into(),
        "length" | "max_tokens" => "length".into(),
        _ => "stop".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(events: &[CanonicalEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                CanonicalEvent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn reasoning_of(events: &[CanonicalEvent]) -> String {
        events
            .iter()
            .filter_map(|event| match event {
                CanonicalEvent::Reasoning { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cumulative_snapshots_are_turned_into_increments() {
        let decoder = Decoder::default();
        let first = decoder.decode("data: {\"response\":\"你\"}");
        assert_eq!(text_of(&first), "你");
        // 累计快照：只吐新增的"好"。
        let second = decoder.decode("data: {\"response\":\"你好\"}");
        assert_eq!(text_of(&second), "好");
        // 重复下发同一份快照：不吐任何东西（否则客户端会重复显示）。
        let third = decoder.decode("data: {\"response\":\"你好\"}");
        assert!(text_of(&third).is_empty());
        assert!(third.is_empty());
    }

    #[test]
    fn reasoning_is_kept_out_of_content() {
        let decoder = Decoder::default();
        let events = decoder.decode("data: {\"reasoning_content\":\"想一想\"}");
        assert_eq!(reasoning_of(&events), "想一想");
        assert!(text_of(&events).is_empty(), "思考不能混进正文");

        let nested = decoder.decode(
            "data: {\"message\":{\"content\":[{\"type\":\"reasoning\",\"text\":\"再想\"},{\"type\":\"text\",\"text\":\"答案\"}]}}",
        );
        assert_eq!(reasoning_of(&nested), "再想");
        assert_eq!(text_of(&nested), "答案");
    }

    #[test]
    fn event_name_variants_all_normalize() {
        assert_eq!(normalize_event_name("ResponseDone", &Value::Null), "done");
        assert_eq!(normalize_event_name("tokenUsage", &Value::Null), "token_usage");
        assert_eq!(
            normalize_event_name("", &serde_json::json!({"event": "plan_item"})),
            "plan_item"
        );
        // 未知事件名原样保留（解码层不认识的会返回空，不会中断流）。
        assert_eq!(normalize_event_name("some_new_thing", &Value::Null), "some_new_thing");
    }

    #[test]
    /// `plan_item` 才是正文的真实载体。
    ///
    /// 真实帧（抓包）：
    /// ```text
    /// event: plan_item
    /// data: {"id":"pi_...","thought":"你好","reasoning_content":"",
    ///        "tool_call_info":{"id":"","name":"","params":null}}
    /// ```
    /// 同一个 plan item 会**重复下发**（tool_call_started / finished 各一次），
    /// 所以必须按累计快照处理，不能每帧都追加。
    #[test]
    fn plan_item_carries_the_visible_answer() {
        let decoder = Decoder::default();
        let first = decoder.decode(
            "event: plan_item\ndata: {\"id\":\"pi_1\",\"thought\":\"你\",\"reasoning_content\":\"\",\"tool_call_info\":{\"name\":\"\"}}",
        );
        assert_eq!(text_of(&first), "你");
        // 同一 id 重复下发同一份快照：不重复吐。
        let replay = decoder.decode(
            "event: plan_item\ndata: {\"id\":\"pi_1\",\"thought\":\"你\",\"reasoning_content\":\"\",\"tool_call_info\":{\"name\":\"\"}}",
        );
        assert!(text_of(&replay).is_empty(), "累计快照不该重复");
        // 快照变长：只吐新增。
        let grown = decoder.decode(
            "event: plan_item\ndata: {\"id\":\"pi_1\",\"thought\":\"你好\",\"reasoning_content\":\"\",\"tool_call_info\":{\"name\":\"\"}}",
        );
        assert_eq!(text_of(&grown), "好");
    }

    /// 计划项同时带 `thought` 与 `content` 时要拼接，且不重复。
    #[test]
    fn plan_item_joins_thought_and_content_without_duplication() {
        let decoder = Decoder::default();
        let both = decoder.decode(
            "event: plan_item\ndata: {\"thought\":\"思考\",\"content\":\"答案\"}",
        );
        assert_eq!(text_of(&both), "思考\n\n答案");

        // content 已包含在 thought 里：只留 thought。
        let nested = Decoder::default().decode(
            "event: plan_item\ndata: {\"thought\":\"思考里有答案\",\"content\":\"答案\"}",
        );
        assert_eq!(text_of(&nested), "思考里有答案");
    }

    /// `finish` 是终止标记，不是客户端要执行的工具调用。
    #[test]
    fn finish_marker_is_not_emitted_as_a_tool_call() {
        let decoder = Decoder::default();
        let events = decoder.decode(
            "event: plan_item\ndata: {\"thought\":\"你好\",\"tool_call_info\":{\"id\":\"pi_1\",\"name\":\"finish\",\"params\":{\"summary\":\"你好\"}}}",
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, CanonicalEvent::ToolCall { .. })),
            "finish 不能变成 tool_call：{events:?}"
        );
        assert_eq!(text_of(&events), "你好");

        // 普通 tool_call_info 仍然要识别。
        let real = decoder.decode(
            "event: plan_item\ndata: {\"tool_call_info\":{\"name\":\"read_file\",\"id\":\"c1\",\"params\":{\"path\":\"a\"}}}",
        );
        assert!(real
            .iter()
            .any(|event| matches!(event, CanonicalEvent::ToolCall { .. })));
    }

    fn tool_call_info_becomes_a_cir_tool_call() {
        let decoder = Decoder::default();
        let events = decoder.decode(
            "data: {\"tool_call_info\":{\"name\":\"read_file\",\"tool_call_id\":\"call_1\",\"arguments\":{\"path\":\"a\"}}}",
        );
        let call = events
            .iter()
            .find_map(|event| match event {
                CanonicalEvent::ToolCall {
                    id,
                    name,
                    arguments_delta,
                    ..
                } => Some((id.clone(), name.clone(), arguments_delta.clone())),
                _ => None,
            })
            .expect("必须识别出工具调用");
        assert_eq!(call.0.as_deref(), Some("call_1"));
        assert_eq!(call.1.as_deref(), Some("read_file"));
        assert_eq!(call.2.as_deref(), Some("{\"path\":\"a\"}"));
    }

    #[test]
    fn error_frame_surfaces_as_error_event() {
        let decoder = Decoder::default();
        let events = decoder.decode("event: error\ndata: {\"message\":\"上游炸了\"}");
        assert!(matches!(
            &events[0],
            CanonicalEvent::Error { message } if message == "上游炸了"
        ));
    }

    #[test]
    fn done_emits_finish_once_with_usage() {
        let decoder = Decoder::default();
        let events = decoder.decode(
            "event: done\ndata: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}",
        );
        assert!(events.iter().any(|event| matches!(event, CanonicalEvent::Usage(usage) if usage.total_tokens == 12)));
        assert!(events.iter().any(|event| matches!(event, CanonicalEvent::Finish { reason } if reason == "stop")));
        // 之后的 finish() 不该再吐第二个 Finish（编码器会多写一帧）。
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn finish_is_synthesised_when_upstream_never_sends_done() {
        let decoder = Decoder::default();
        decoder.decode("data: {\"response\":\"半句话\"}");
        let tail = decoder.finish();
        assert!(matches!(&tail[..], [CanonicalEvent::Finish { .. }]));
        assert!(decoder.finish().is_empty(), "只补一次");
    }

    #[test]
    fn queue_notice_is_visible_text() {
        let decoder = Decoder::default();
        let events = decoder.decode("event: request_wait_in_queue\ndata: {\"position\":3}");
        assert!(text_of(&events).contains("排队中"));
    }

    #[test]
    fn push_delta_never_shrinks_output() {
        let mut buffer = String::from("你好世界");
        // 更短的快照：不吐、也不回退。
        assert_eq!(push_delta(&mut buffer, "你好"), None);
        assert_eq!(buffer, "你好世界");
        // 变长但不是前缀：只吐尾部新增。
        assert_eq!(push_delta(&mut buffer, "你好世界啊"), Some("啊".into()));
    }
}
