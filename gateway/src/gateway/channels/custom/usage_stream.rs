//! Anthropic 的 usage 分布在 message_start 和 message_delta 中。
//! 只覆盖上游本帧实际给出的字段，缺字段保留前值，显式 0 仍然生效。

use futures_util::StreamExt;
use serde_json::{Map, Value};

use crate::gateway::transport::FrameStream;

pub(super) fn normalize_anthropic_usage(frames: FrameStream) -> FrameStream {
    let mut usage = Map::new();
    Box::pin(frames.map(move |frame| {
        frame.map(|frame| {
            let Ok(mut value) = serde_json::from_str::<Value>(&frame) else {
                return frame;
            };
            let slot = match value.get("type").and_then(Value::as_str) {
                Some("message_start") => {
                    usage.clear();
                    value
                        .get_mut("message")
                        .and_then(|message| message.get_mut("usage"))
                }
                Some("message_delta") => value.get_mut("usage"),
                _ => return frame,
            };
            let Some(fields) = slot.and_then(Value::as_object_mut) else {
                return frame;
            };
            usage.extend(fields.clone());
            *fields = usage.clone();
            value.to_string()
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use serde_json::json;

    use crate::gateway::{
        channels::{custom::CustomChannel, ChannelAdapter},
        protocol::{cir::CanonicalEvent, encode},
    };

    fn frames(values: Vec<Value>) -> FrameStream {
        Box::pin(stream::iter(
            values.into_iter().map(|value| Ok(value.to_string())),
        ))
    }

    #[tokio::test]
    async fn partial_usage_survives_through_all_response_protocols() {
        let adapter = CustomChannel::from_options("test", &json!({"protocol":"anthropic"}));
        let mut frames = adapter.normalize_stream(frames(vec![
            json!({"type":"message_start","message":{"usage":{
                "input_tokens":20,"cache_read_input_tokens":8192,
                "cache_creation_input_tokens":100,"output_tokens":0
            }}}),
            json!({"type":"message_delta","usage":{"output_tokens":3}}),
            json!({"type":"message_delta","usage":{"output_tokens":7},
                "delta":{"stop_reason":"end_turn"}}),
        ]));
        let mut events = Vec::new();
        let mut streaming = encode::anthropic::StreamEncoder::new("msg", "model", 172_200);
        while let Some(frame) = frames.next().await {
            for event in adapter.parse_event(&frame.expect("frame")) {
                streaming.push(&event);
                events.push(event);
            }
        }

        // 三个入口落库都读取最后一条 Usage，它必须是完整快照。
        let usage = events
            .iter()
            .rev()
            .find_map(|event| match event {
                CanonicalEvent::Usage(usage) => Some(usage),
                _ => None,
            })
            .expect("upstream usage");
        assert_eq!(usage.prompt_tokens, 8312);
        assert_eq!(usage.cached_tokens, 8192);
        assert_eq!(usage.cache_creation_tokens, 100);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 8319);

        let chat = encode::openai::fold(&events, "id", "model", 0).expect("chat");
        assert_eq!(chat["usage"]["prompt_tokens"], 8312);
        assert_eq!(
            chat["usage"]["prompt_tokens_details"]["cached_tokens"],
            8192
        );
        let responses = encode::responses::fold(&events, "id", "model", 0, 0).expect("responses");
        assert_eq!(responses["usage"]["input_tokens"], 8312);
        assert_eq!(
            responses["usage"]["input_tokens_details"]["cached_tokens"],
            8192
        );
        let message = encode::anthropic::fold(&events, "id", "model").expect("message");
        let expected = json!({"input_tokens":20,"cache_read_input_tokens":8192,
            "cache_creation_input_tokens":100,"output_tokens":7});
        assert_eq!(message["usage"], expected);
        let final_frames = streaming.finish("stop");
        let delta: Value = serde_json::from_str(
            std::str::from_utf8(&final_frames[0])
                .expect("utf8")
                .split_once("data: ")
                .expect("data")
                .1
                .trim(),
        )
        .expect("delta");
        assert_eq!(delta["usage"], expected);
    }

    #[tokio::test]
    async fn omitted_fields_are_preserved_but_explicit_zero_overrides() {
        let mut frames = normalize_anthropic_usage(frames(vec![
            json!({"type":"message_start","message":{"usage":{
                "input_tokens":20,"cache_read_input_tokens":8192,"output_tokens":0
            }}}),
            json!({"type":"message_delta","usage":{"output_tokens":3}}),
            json!({"type":"message_delta","usage":{"cache_read_input_tokens":0,"output_tokens":2}}),
        ]));
        frames.next().await;
        let partial: Value = serde_json::from_str(&frames.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(partial["usage"]["cache_read_input_tokens"], 8192);
        let corrected: Value =
            serde_json::from_str(&frames.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(corrected["usage"]["input_tokens"], 20);
        assert_eq!(corrected["usage"]["cache_read_input_tokens"], 0);
        assert_eq!(corrected["usage"]["output_tokens"], 2);
    }

    #[tokio::test]
    async fn requests_do_not_share_usage_and_other_frames_pass_through() {
        let mut first = normalize_anthropic_usage(frames(vec![
            json!({"type":"message_start","message":{"usage":{"cache_read_input_tokens":8192}}}),
            json!({"type":"message_delta","usage":{"output_tokens":1}}),
        ]));
        let mut second = normalize_anthropic_usage(frames(vec![
            json!({"type":"message_delta","usage":{"output_tokens":2}}),
            json!({"choices":[],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":64}}}),
        ]));
        first.next().await;
        let other: Value = serde_json::from_str(&second.next().await.unwrap().unwrap()).unwrap();
        assert!(other["usage"].get("cache_read_input_tokens").is_none());
        let original = json!({"choices":[],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":64}}}).to_string();
        assert_eq!(second.next().await.unwrap().unwrap(), original);
        let retained: Value = serde_json::from_str(&first.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(retained["usage"]["cache_read_input_tokens"], 8192);
    }
}
