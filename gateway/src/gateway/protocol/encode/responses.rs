//! CIR 事件 → Responses 事件序列与非流式 response。
//!
//! Responses 的流是"带事件名的 SSE"：`response.created` → 逐个 `output_item` 的
//! `.added`/`.delta`/`.done` → `response.completed`；非流式则是一个带 `output` 数组的对象。

use std::collections::BTreeMap;

use axum::body::Bytes;
use serde_json::{json, Value};

use crate::gateway::errors::GatewayError;
use crate::gateway::protocol::cir::{CanonicalEvent, ToolCall, Usage};

pub fn event_frame(name: &str, payload: &Value) -> Bytes {
    Bytes::from(format!("event: {name}\ndata: {payload}\n\n"))
}

pub fn usage_json(usage: &Usage) -> Value {
    json!({
        "input_tokens": usage.prompt_tokens,
        "input_tokens_details": { "cached_tokens": usage.cached_tokens },
        "output_tokens": usage.completion_tokens,
        "output_tokens_details": { "reasoning_tokens": usage.reasoning_tokens },
        "total_tokens": usage.total_tokens,
    })
}

/// `finish_reason=length` 时整条 response 是 incomplete。
pub fn status_for(finish: &str) -> &'static str {
    match finish {
        "length" | "max_tokens" => "incomplete",
        _ => "completed",
    }
}

/// 事件累积器：流式收尾与非流式折叠共用同一份语义。
#[derive(Default)]
struct Accumulator {
    reasoning: String,
    text: String,
    tool_calls: BTreeMap<u32, ToolCall>,
    finish: String,
    usage: Usage,
}

impl Accumulator {
    fn apply(&mut self, event: &CanonicalEvent) -> Result<(), GatewayError> {
        match event {
            CanonicalEvent::Text { text } => self.text.push_str(text),
            CanonicalEvent::Reasoning { text } => self.reasoning.push_str(text),
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let entry = self.tool_calls.entry(*index).or_default();
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
            CanonicalEvent::Usage(usage) => self.usage = usage.clone(),
            CanonicalEvent::Finish { reason } => self.finish = reason.clone(),
            CanonicalEvent::Error { message } => {
                return Err(GatewayError::upstream(502, message.clone()))
            }
        }
        Ok(())
    }

    /// 组装 `output` 数组：reasoning item → function_call items → message item。
    fn output(&self, response_id: &str) -> Vec<Value> {
        let mut output = Vec::new();
        if !self.reasoning.is_empty() {
            output.push(json!({
                "type": "reasoning",
                "id": format!("rs_{response_id}"),
                "summary": [{ "type": "summary_text", "text": self.reasoning }],
            }));
        }
        for (index, call) in &self.tool_calls {
            output.push(json!({
                "type": "function_call",
                "id": format!("fc_{response_id}_{index}"),
                "call_id": call.id.clone().unwrap_or_else(|| format!("call_{index}")),
                "name": call.name,
                "arguments": call.arguments,
                "status": "completed",
            }));
        }
        if !self.text.is_empty() {
            output.push(json!({
                "type": "message",
                "id": format!("msg_{response_id}"),
                "role": "assistant",
                "status": "completed",
                "content": [{ "type": "output_text", "text": self.text, "annotations": [] }],
            }));
        }
        output
    }
}

/// 取某个 tool call 的 output_index（编码器内部已在 map 里维护）。
fn output_index_of(encoder: &StreamEncoder, call_index: u32) -> u32 {
    encoder
        .tool_items
        .get(&call_index)
        .map(|(_, output_index)| *output_index)
        .unwrap_or(call_index)
}

/// 有状态的流式编码器：负责 output_item 的 added / done 开合。
pub struct StreamEncoder {
    response_id: String,
    model: String,
    created_at: u64,
    input_tokens: u64,
    state: Accumulator,
    output_index: u32,
    reasoning_item: Option<(String, u32)>,
    message_item: Option<(String, u32)>,
    tool_items: BTreeMap<u32, (String, u32)>,
}

impl StreamEncoder {
    pub fn new(
        response_id: impl Into<String>,
        model: impl Into<String>,
        created_at: u64,
        input_tokens: u64,
    ) -> Self {
        Self {
            response_id: response_id.into(),
            model: model.into(),
            created_at,
            input_tokens,
            state: Accumulator::default(),
            output_index: 0,
            reasoning_item: None,
            message_item: None,
            tool_items: BTreeMap::new(),
        }
    }

    fn skeleton(&self) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "model": self.model,
            "status": "in_progress",
            "output": [],
        })
    }

    pub fn start(&self) -> Vec<Bytes> {
        vec![
            event_frame("response.created", &json!({ "type": "response.created", "response": self.skeleton() })),
            event_frame(
                "response.in_progress",
                &json!({ "type": "response.in_progress", "response": self.skeleton() }),
            ),
        ]
    }

    pub fn push(&mut self, event: &CanonicalEvent) -> Vec<Bytes> {
        // 先累积语义，再按需要补 item 的开帧；错误事件直接透出。
        if let Err(error) = self.state.apply(event) {
            return vec![event_frame(
                "error",
                &json!({ "type": "error", "message": error.message }),
            )];
        }
        match event {
            CanonicalEvent::Text { text } => {
                let (item_id, index) = match &self.message_item {
                    Some(entry) => entry.clone(),
                    None => {
                        let index = self.next_index();
                        let item_id = format!("msg_{}_{index}", self.response_id);
                        self.message_item = Some((item_id.clone(), index));
                        return vec![
                            event_frame(
                                "response.output_item.added",
                                &json!({ "type": "response.output_item.added", "output_index": index,
                                         "item": { "type": "message", "id": item_id, "role": "assistant",
                                                   "status": "in_progress", "content": [] } }),
                            ),
                            event_frame(
                                "response.output_text.delta",
                                &json!({ "type": "response.output_text.delta", "item_id": item_id,
                                         "output_index": index, "content_index": 0, "delta": text }),
                            ),
                        ];
                    }
                };
                vec![event_frame(
                    "response.output_text.delta",
                    &json!({ "type": "response.output_text.delta", "item_id": item_id,
                             "output_index": index, "content_index": 0, "delta": text }),
                )]
            }
            CanonicalEvent::Reasoning { text } => {
                let (item_id, index) = match &self.reasoning_item {
                    Some(entry) => entry.clone(),
                    None => {
                        let index = self.next_index();
                        let item_id = format!("rs_{}_{index}", self.response_id);
                        self.reasoning_item = Some((item_id.clone(), index));
                        // AI SDK 客户端（ZCode / Codex 等）在 part 上维护 summaryParts：
                        // 没有 part.added，收尾清理时读 summaryParts 会直接崩
                        // （TypeError: reading 'summaryParts'）。
                        return vec![
                            event_frame(
                                "response.output_item.added",
                                &json!({ "type": "response.output_item.added", "output_index": index,
                                         "item": { "type": "reasoning", "id": item_id, "summary": [] } }),
                            ),
                            event_frame(
                                "response.reasoning_summary_part.added",
                                &json!({ "type": "response.reasoning_summary_part.added",
                                         "item_id": item_id, "output_index": index,
                                         "summary_index": 0,
                                         "part": { "type": "summary_text", "text": "" } }),
                            ),
                            event_frame(
                                "response.reasoning_summary_text.delta",
                                &json!({ "type": "response.reasoning_summary_text.delta",
                                         "item_id": item_id, "output_index": index,
                                         "summary_index": 0, "delta": text }),
                            ),
                        ];
                    }
                };
                vec![event_frame(
                    "response.reasoning_summary_text.delta",
                    &json!({ "type": "response.reasoning_summary_text.delta", "item_id": item_id,
                             "output_index": index, "summary_index": 0, "delta": text }),
                )]
            }
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => {
                // call_id 必须全流一致：`output_item.added` 里就定下来，
                // 收尾的 `output_item.done` 与 `response.completed` 复用同一个值。
                // 之前 added 发 `call_{index}` 占位、done 才换成上游真 id，
                // 客户端（本仓库 agent 的 StreamDecoder 等）按 added 的 id
                // 建立索引，done 换 id 后找不到关联事件 → "工具块缺少关联事件"。
                let (item_id, output_index, _call_id) = match self.tool_items.get(index) {
                    Some(entry) => {
                        let (item_id, output_index) = entry.clone();
                        let call_id = self
                            .state
                            .tool_calls
                            .get(index)
                            .and_then(|call| call.id.clone())
                            .unwrap_or_else(|| format!("call_{index}"));
                        (item_id, output_index, call_id)
                    }
                    None => {
                        let output_index = self.next_index();
                        let item_id = format!("fc_{}_{index}", self.response_id);
                        let call_id = id
                            .clone()
                            .or_else(|| {
                                self.state.tool_calls.get(index).and_then(|call| call.id.clone())
                            })
                            .unwrap_or_else(|| format!("call_{index}"));
                        self.tool_items
                            .insert(*index, (item_id.clone(), output_index));
                        let mut frames = vec![event_frame(
                            "response.output_item.added",
                            &json!({ "type": "response.output_item.added", "output_index": output_index,
                                     "item": { "type": "function_call", "id": item_id,
                                               "call_id": call_id,
                                               "name": name.clone().unwrap_or_default(),
                                               "arguments": "", "status": "in_progress" } }),
                        )];
                        if let Some(delta) = arguments_delta {
                            frames.push(event_frame(
                                "response.function_call_arguments.delta",
                                &json!({ "type": "response.function_call_arguments.delta",
                                         "item_id": item_id, "output_index": output_index,
                                         "delta": delta }),
                            ));
                        }
                        return frames;
                    }
                };
                match arguments_delta {
                    Some(delta) => vec![event_frame(
                        "response.function_call_arguments.delta",
                        &json!({ "type": "response.function_call_arguments.delta", "item_id": item_id,
                                 "output_index": output_index, "delta": delta }),
                    )],
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    /// 收尾：每个开着的 item 都要**按协议顺序**补齐 done 事件，再发终态。
    ///
    /// reasoning：summary_text.done → summary_part.done → output_item.done；
    /// message：output_text.done → output_item.done；
    /// function_call：output_item.done。
    /// 缺任何一个，AI SDK 的流就没"走完"，iterator 清理时会崩。
    pub fn finish(&mut self, finish: &str) -> Vec<Bytes> {
        let output = self.state.output(&self.response_id);
        let mut frames = Vec::new();

        // reasoning item 的收尾（它一定是第一个开出来的 item）。
        if let Some((item_id, index)) = self.reasoning_item.take() {
            let summary = self
                .state
                .reasoning
                .clone();
            frames.push(event_frame(
                "response.reasoning_summary_text.done",
                &json!({ "type": "response.reasoning_summary_text.done", "item_id": item_id,
                         "output_index": index, "summary_index": 0, "text": summary }),
            ));
            frames.push(event_frame(
                "response.reasoning_summary_part.done",
                &json!({ "type": "response.reasoning_summary_part.done", "item_id": item_id,
                         "output_index": index, "summary_index": 0,
                         "part": { "type": "summary_text", "text": summary } }),
            ));
            frames.push(event_frame(
                "response.output_item.done",
                &json!({ "type": "response.output_item.done", "output_index": index,
                         "item": { "type": "reasoning", "id": item_id,
                                   "summary": [{ "type": "summary_text", "text": summary }] } }),
            ));
        }

        // message item 的收尾。
        if let Some((item_id, index)) = self.message_item.take() {
            let text = self.state.text.clone();
            frames.push(event_frame(
                "response.output_text.done",
                &json!({ "type": "response.output_text.done", "item_id": item_id,
                         "output_index": index, "content_index": 0, "text": text }),
            ));
            frames.push(event_frame(
                "response.output_item.done",
                &json!({ "type": "response.output_item.done", "output_index": index,
                         "item": { "type": "message", "id": item_id, "role": "assistant",
                                   "status": "completed",
                                   "content": [{ "type": "output_text", "text": text,
                                                 "annotations": [] }] } }),
            ));
        }

        // function_call items 的收尾（按 output_index 顺序）。
        let mut tools: Vec<(u32, String)> = self
            .tool_items
            .iter()
            .map(|(index, (item_id, output_index))| (*output_index, format!("{item_id}|{index}")))
            .collect();
        tools.sort();
        for (_, pair) in tools {
            let (item_id, raw_index) = pair.split_once('|').unwrap_or(("", "0"));
            let call_index: u32 = raw_index.parse().unwrap_or(0);
            let call = self.state.tool_calls.get(&call_index);
            let (name, arguments, call_id) = match call {
                Some(call) => (call.name.clone(), call.arguments.clone(), call.id.clone()),
                None => (String::new(), String::new(), None),
            };
            frames.push(event_frame(
                "response.function_call_arguments.done",
                &json!({ "type": "response.function_call_arguments.done", "item_id": item_id,
                         "output_index": output_index_of(&self, call_index), "arguments": arguments }),
            ));
            frames.push(event_frame(
                "response.output_item.done",
                &json!({ "type": "response.output_item.done",
                         "output_index": output_index_of(&self, call_index),
                         "item": { "type": "function_call", "id": item_id,
                                   "call_id": call_id.unwrap_or_else(|| format!("call_{call_index}")),
                                   "name": name, "arguments": arguments,
                                   "status": "completed" } }),
            ));
        }
        self.tool_items.clear();

        let mut response = self.skeleton();
        response["status"] = json!(status_for(finish));
        response["output"] = json!(output);
        response["usage"] = usage_json(&Usage {
            prompt_tokens: self.state.usage.prompt_tokens.max(self.input_tokens),
            ..self.state.usage.clone()
        });
        frames.push(event_frame(
            "response.completed",
            &json!({ "type": "response.completed", "response": response }),
        ));
        frames
    }

    fn next_index(&mut self) -> u32 {
        let index = self.output_index;
        self.output_index += 1;
        index
    }
}

/// 非流式：整条事件流 → 一个 response 对象。
pub fn fold(
    events: &[CanonicalEvent],
    response_id: &str,
    model: &str,
    created_at: u64,
    input_tokens: u64,
) -> Result<Value, GatewayError> {
    let mut state = Accumulator::default();
    for event in events {
        state.apply(event)?;
    }
    let finish = state.finish.clone();
    let output = state.output(response_id);
    Ok(json!({
        "id": response_id,
        "object": "response",
        "created_at": created_at,
        "model": model,
        "status": status_for(&finish),
        "output": output,
        "usage": usage_json(&Usage {
            prompt_tokens: state.usage.prompt_tokens.max(input_tokens),
            ..state.usage
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(frames: &[Bytes]) -> String {
        frames
            .iter()
            .map(|frame| String::from_utf8_lossy(frame).to_string())
            .collect()
    }

    #[test]
    fn streaming_emits_item_lifecycle_and_completion() {
        let mut encoder = StreamEncoder::new("resp_1", "glm-5.3", 7, 9);
        let mut out = text_of(&encoder.start());
        out.push_str(&text_of(&encoder.push(&CanonicalEvent::Reasoning {
            text: "想".into(),
        })));
        out.push_str(&text_of(&encoder.push(&CanonicalEvent::Text {
            text: "你好".into(),
        })));
        out.push_str(&text_of(&encoder.push(&CanonicalEvent::Text {
            text: "，世界".into(),
        })));
        out.push_str(&text_of(&encoder.finish("stop")));

        assert!(out.contains("event: response.created"), "{out}");
        assert!(out.contains("response.output_item.added"), "{out}");
        assert!(out.contains("response.reasoning_summary_text.delta"), "{out}");
        assert!(out.contains("response.output_text.delta"), "{out}");
        assert!(out.contains("response.output_item.done"), "{out}");
        assert!(out.contains("event: response.completed"), "{out}");
        assert!(out.contains("\"text\":\"你好，世界\""), "{out}");
        // 文本 item 只 added 一次：added + done + completed 里各出现一次，共 3 次。
        // 多个 delta 复用同一个 item，不会重复 added。
        assert_eq!(out.matches("\"type\":\"message\"").count(), 3, "{out}");
        // 用事件名计数，避免把 payload 里的 type 字段也算进去。
        assert_eq!(
            out.matches("event: response.output_item.added").count(),
            2,
            "{out}"
        );
    }

    #[test]
    fn finish_reason_length_marks_incomplete() {
        assert_eq!(status_for("length"), "incomplete");
        assert_eq!(status_for("stop"), "completed");
        let mut encoder = StreamEncoder::new("resp_2", "glm-5.3", 0, 0);
        let out = text_of(&encoder.finish("length"));
        assert!(out.contains("\"status\":\"incomplete\""), "{out}");
    }

    #[test]
    fn stream_closes_every_item_with_done_events_in_order() {
        // 回归：AI SDK（ZCode / Codex）要求每个 item 都有配对的收尾事件，
        // reasoning 还要带 part 级事件 —— 缺了会在 iterator 清理时崩
        // （Cannot read properties of undefined (reading 'summaryParts')）。
        let mut encoder = StreamEncoder::new("resp_x", "glm-5.3", 0, 0);
        let mut out = String::new();
        let push = |frames: Vec<Bytes>, out: &mut String| {
            for frame in frames {
                out.push_str(&String::from_utf8_lossy(&frame));
            }
        };
        push(encoder.start(), &mut out);
        push(encoder.push(&CanonicalEvent::Reasoning { text: "想".into() }), &mut out);
        push(encoder.push(&CanonicalEvent::Text { text: "答".into() }), &mut out);
        push(encoder.finish("stop"), &mut out);

        let part_added = out.matches("event: response.reasoning_summary_part.added").count();
        let part_done = out.matches("event: response.reasoning_summary_part.done").count();
        let text_done = out.matches("event: response.output_text.done").count();
        let reasoning_done = out.matches("event: response.reasoning_summary_text.done").count();
        assert_eq!(part_added, 1, "part.added 缺失会让客户端崩：{out}");
        assert_eq!(part_done, 1, "part.done 缺失会让客户端崩：{out}");
        assert_eq!(reasoning_done, 1, "{out}");
        assert_eq!(text_done, 1, "{out}");
        assert!(out.contains("event: response.completed"), "{out}");

        // 顺序：part.done 必须在 reasoning item 的 output_item.done 之前。
        let part_done_at = out.find("response.reasoning_summary_part.done").unwrap();
        let item_done_at = out.find("event: response.output_item.done").unwrap();
        assert!(part_done_at < item_done_at, "part.done 要先于 item.done：{out}");
        // text 的 done 在 reasoning 的 item.done 之后（item 顺序不能交叉）。
        let text_done_at = out.find("response.output_text.done").unwrap();
        assert!(item_done_at < text_done_at, "{out}");
    }

    #[test]
    fn tool_calls_become_function_call_items_with_deltas() {
        let mut encoder = StreamEncoder::new("resp_3", "glm-5.3", 0, 0);
        let mut out = text_of(&encoder.push(&CanonicalEvent::ToolCall {
            index: 0,
            id: Some("call_1".into()),
            name: Some("get_weather".into()),
            arguments_delta: Some("{\"city\"".into()),
        }));
        out.push_str(&text_of(&encoder.push(&CanonicalEvent::ToolCall {
            index: 0,
            id: None,
            name: None,
            arguments_delta: Some(":\"上海\"}".into()),
        })));
        out.push_str(&text_of(&encoder.finish("tool_calls")));

        assert!(out.contains("\"type\":\"function_call\""), "{out}");
        assert!(out.contains("response.function_call_arguments.delta"), "{out}");
        // 两个分片要在最终 item 里拼成一个完整 JSON。
        assert!(out.contains("{\\\"city\\\":\\\"上海\\\"}"), "{out}");
    }

    /// 回归：call_id 必须全流一致。added 里发占位 `call_0`、done 里才换成
    /// 上游真 id 的话，客户端按 added 的 id 索引工具块，done 换 id 后
    /// 找不到关联事件（agent 报"工具块缺少关联事件"）。
    #[test]
    fn tool_call_id_is_identical_in_added_and_done() {
        let mut encoder = StreamEncoder::new("resp_6", "glm-5.3", 0, 0);
        let mut out = text_of(&encoder.push(&CanonicalEvent::ToolCall {
            index: 0,
            id: Some("call_714038bbf06a4438b248c2b6".into()),
            name: Some("files__list".into()),
            arguments_delta: Some("{\"path\":\\\"D:\\\\code\\\"}".into()),
        }));
        out.push_str(&text_of(&encoder.push(&CanonicalEvent::ToolCall {
            index: 0,
            id: None,
            name: None,
            arguments_delta: None,
        })));
        out.push_str(&text_of(&encoder.finish("tool_calls")));

        assert!(out.contains("\"call_id\":\"call_714038bbf06a4438b248c2b6\""), "{out}");
        assert!(!out.contains("\"call_id\":\"call_0\""), "占位 id 泄漏进流：{out}");
        // added 与 done 里的 call_id 必须是同一个值。
        let added = out.split("response.output_item.done").next().unwrap_or("");
        assert!(
            added.contains("call_714038bbf06a4438b248c2b6"),
            "added 帧没带真实 call_id：{added}"
        );
    }

    #[test]
    fn non_streaming_fold_returns_response_object() {
        let events = vec![
            CanonicalEvent::Reasoning { text: "想".into() },
            CanonicalEvent::Text { text: "答案".into() },
            CanonicalEvent::Usage(Usage {
                prompt_tokens: 11,
                completion_tokens: 4,
                reasoning_tokens: 2,
                cached_tokens: 6,
                total_tokens: 15,
                ..Usage::default()
            }),
            CanonicalEvent::Finish { reason: "stop".into() },
        ];
        let response = fold(&events, "resp_4", "glm-5.3", 1, 0).expect("折叠成功");
        assert_eq!(response["object"], "response");
        assert_eq!(response["status"], "completed");
        assert_eq!(response["output"][0]["type"], "reasoning");
        assert_eq!(response["output"][1]["type"], "message");
        assert_eq!(response["output"][1]["content"][0]["text"], "答案");
        assert_eq!(response["usage"]["input_tokens"], 11);
        assert_eq!(response["usage"]["input_tokens_details"]["cached_tokens"], 6);
        assert_eq!(response["usage"]["output_tokens_details"]["reasoning_tokens"], 2);
    }

    #[test]
    fn upstream_error_event_surfaces_instead_of_fake_response() {
        let events = vec![CanonicalEvent::Error {
            message: "断流".into(),
        }];
        assert!(fold(&events, "resp_5", "m", 0, 0).is_err());
    }
}
