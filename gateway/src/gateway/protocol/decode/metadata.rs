//! 三种协议共用会话字段解析，避免入口丢失客户端提供的会话锚点。

use serde_json::{Map, Value};

use crate::gateway::protocol::cir::RequestMetadata;

pub(super) fn decode_metadata(object: &Map<String, Value>) -> RequestMetadata {
    let nested = object.get("metadata").and_then(Value::as_object);
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let fields = ["conversation_id", "session_id", "prompt_cache_key"];
    RequestMetadata {
        session_id: fields
            .iter()
            .find_map(|key| text(object.get(*key)))
            .or_else(|| nested.and_then(|map| fields.iter().find_map(|key| text(map.get(*key))))),
        user_id: nested
            .and_then(|map| text(map.get("user_id")))
            .or_else(|| text(object.get("user"))),
        header_session: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::{cir::CanonicalRequest, decode};
    use serde_json::json;

    fn decode_all(extra: Value) -> Vec<CanonicalRequest> {
        let mut body = json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}],
            "input": "hello",
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        vec![
            decode::openai::decode_chat(&body).unwrap(),
            decode::responses::decode_response(&body).unwrap(),
            decode::anthropic::decode_messages(&body).unwrap(),
        ]
    }

    #[test]
    fn every_protocol_reads_top_level_and_nested_session_fields() {
        for field in ["conversation_id", "session_id", "prompt_cache_key"] {
            for nested in [false, true] {
                let field_value = json!({field: " conversation-1 "});
                let extra = if nested {
                    json!({"metadata": field_value})
                } else {
                    field_value
                };
                for request in decode_all(extra) {
                    assert_eq!(
                        request.metadata.session_key().as_deref(),
                        Some("conversation-1")
                    );
                }
            }
        }
    }

    #[test]
    fn blank_and_non_string_fields_do_not_hide_valid_lower_priority_fields() {
        for request in decode_all(json!({
            "conversation_id": "  ", "session_id": 42,
            "metadata": {"conversation_id": "nested-session", "user_id": "user-1"},
        })) {
            assert_eq!(
                request.metadata.session_key().as_deref(),
                Some("nested-session")
            );
        }
        for request in decode_all(json!({
            "conversation_id": "", "prompt_cache_key": "cache-session",
            "metadata": {"session_id": "nested-session"},
        })) {
            assert_eq!(
                request.metadata.session_key().as_deref(),
                Some("cache-session")
            );
        }
    }

    #[test]
    fn explicit_session_wins_over_user_fallback_in_every_protocol() {
        for request in decode_all(json!({
            "session_id": "session-1", "user": "legacy-user",
            "metadata": {"user_id": "metadata-user"},
        })) {
            assert_eq!(request.metadata.session_key().as_deref(), Some("session-1"));
        }
        for request in decode_all(json!({"user": "legacy-user"})) {
            assert_eq!(
                request.metadata.session_key().as_deref(),
                Some("legacy-user")
            );
        }
        for request in decode_all(json!({"metadata": {"user_id": "metadata-user"}})) {
            assert_eq!(
                request.metadata.session_key().as_deref(),
                Some("metadata-user")
            );
        }
    }
}
