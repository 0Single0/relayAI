use super::*;
use crate::gateway::{
    channels::{workbuddy::WorkbuddyChannel, AccountSnapshot, ChannelAdapter},
    protocol::{cir::Block, decode},
};
use serde_json::json;

fn chat(text: &str) -> CanonicalRequest {
    decode::openai::decode_chat(&json!({
        "model": "glm-5.3",
        "messages": [{"role": "user", "content": text}],
    }))
    .unwrap()
}

fn binding() -> Binding {
    Binding {
        channel_id: "wb-cn".into(),
        uid: "account-1".into(),
        upstream_model: "glm-5.3".into(),
        expires_at_ms: 0,
        hits: 0,
    }
}

#[test]
fn fallback_session_survives_changing_system_tools_and_long_history() {
    let store = AffinityStore::new(&AffinityConfig::default());
    let first = chat("Inspect this project");
    let first_keys = AffinityStore::candidate_keys("key-1", &first.model, &first);
    let session = AffinityStore::resolve_request("key-1", &first)
        .metadata
        .session_key();
    let now = now_ms();
    store.bind(first_keys[0].clone(), binding(), now);

    let mut next = first.clone();
    next.system = vec![Block::text("Current time and runtime instructions changed")];
    next.tools = decode::openai::decode_chat(&json!({
        "model": "glm-5.3", "messages": [{"role": "user", "content": "x"}],
        "tools": [{"type": "function", "function": {"name": "new_tool"}}],
    }))
    .unwrap()
    .tools;
    for index in 0..100 {
        next.messages
            .extend(chat(&format!("follow-up {index}")).messages);
    }
    let keys = AffinityStore::candidate_keys("key-1", &next.model, &next);
    assert_eq!(keys[0], first_keys[0]);
    assert_eq!(store.lookup(&keys, now).unwrap().1.uid, "account-1");
    assert_eq!(
        AffinityStore::resolve_request("key-1", &next)
            .metadata
            .session_key(),
        session
    );
}

#[test]
fn explicit_session_cannot_inherit_another_sessions_content_binding() {
    let store = AffinityStore::new(&AffinityConfig::default());
    let mut request = chat("same opening");
    let old_content_keys = content_keys("key-1", &request.model, &request);
    store.bind(old_content_keys[0].clone(), binding(), now_ms());
    request.metadata.session_id = Some("new-independent-session".into());
    let keys = AffinityStore::candidate_keys("key-1", &request.model, &request);
    assert_eq!(keys.len(), 1);
    assert!(store.lookup(&keys, now_ms()).is_none());
}

#[test]
fn fallback_can_recover_legacy_content_binding_then_use_a_stable_key() {
    let store = AffinityStore::new(&AffinityConfig::default());
    let first = chat("opening");
    let old_key = content_keys("key-1", &first.model, &first).pop().unwrap();
    store.bind(old_key.clone(), binding(), now_ms());
    let keys = AffinityStore::candidate_keys("key-1", &first.model, &first);
    let (matched, found) = store.lookup(&keys, now_ms()).unwrap();
    assert_eq!(matched, old_key);
    store.bind(keys[0].clone(), found, now_ms());
    store.clear(&old_key);
    let mut next = first.clone();
    next.system = vec![Block::text("updated context")];
    let keys = AffinityStore::candidate_keys("key-1", &next.model, &next);
    assert_eq!(store.lookup(&keys, now_ms()).unwrap().1.uid, "account-1");
}

#[test]
fn stable_fallback_binding_survives_storage_restore() {
    let first = chat("opening");
    let keys = AffinityStore::candidate_keys("key-1", &first.model, &first);
    let before = AffinityStore::new(&AffinityConfig::default());
    before.bind(keys[0].clone(), binding(), now_ms());
    let after = AffinityStore::new(&AffinityConfig::default());
    after.restore(before.snapshot());
    assert_eq!(after.lookup(&keys, now_ms()).unwrap().1.uid, "account-1");
}

#[test]
fn fallback_identity_is_scoped_and_openings_and_images_are_distinct() {
    let first = chat("opening A");
    let key = AffinityStore::fallback_session_key("key-1", &first.model, &first).unwrap();
    assert_ne!(
        Some(&key),
        AffinityStore::fallback_session_key("key-2", &first.model, &first).as_ref()
    );
    assert_ne!(
        Some(&key),
        AffinityStore::fallback_session_key("key-1", "another-model", &first).as_ref()
    );
    assert_ne!(
        Some(&key),
        AffinityStore::fallback_session_key("key-1", &first.model, &chat("opening B")).as_ref()
    );
    let mut with_image = first.clone();
    with_image.messages[0].content.push(Block::Image {
        url: "https://example.test/a.png".into(),
    });
    let image_a = AffinityStore::fallback_session_key("key-1", &first.model, &with_image);
    with_image.messages[0].content[1] = Block::Image {
        url: "https://example.test/b.png".into(),
    };
    assert_ne!(
        image_a,
        AffinityStore::fallback_session_key("key-1", &first.model, &with_image)
    );
}

#[test]
fn resolution_preserves_explicit_identity_and_original_request() {
    let request = chat("opening");
    let resolved = AffinityStore::resolve_request("key-1", &request);
    assert!(resolved.metadata.session_key().is_some());
    assert!(request.metadata.session_key().is_none());
    assert_eq!(request.raw, resolved.raw);
    let mut explicit = request;
    explicit.metadata.header_session = Some("client-session".into());
    assert_eq!(
        AffinityStore::resolve_request("key-1", &explicit)
            .metadata
            .session_key()
            .as_deref(),
        Some("client-session")
    );
}

#[test]
fn explicit_session_survives_replaced_history_and_system() {
    let store = AffinityStore::new(&AffinityConfig::default());
    let mut first = chat("original opening");
    first.metadata.session_id = Some("conversation-1".into());
    let keys = AffinityStore::candidate_keys("key-1", &first.model, &first);
    store.bind(keys[0].clone(), binding(), now_ms());
    let mut next = chat("summary replacing all earlier history");
    next.system = vec![Block::text("new system")];
    next.metadata.session_id = first.metadata.session_id;
    let next_keys = AffinityStore::candidate_keys("key-1", &next.model, &next);
    assert_eq!(next_keys, keys);
    assert_eq!(
        store.lookup(&next_keys, now_ms()).unwrap().1.uid,
        "account-1"
    );
}

#[test]
fn missing_user_opening_does_not_invent_a_shared_fallback_identity() {
    let mut request = chat("");
    request.messages[0].content.clear();
    assert!(AffinityStore::fallback_session_key("key-1", &request.model, &request).is_none());
    request.messages[0].role = Role::Tool;
    request.messages[0].content.push(Block::text("tool result"));
    assert!(AffinityStore::fallback_session_key("key-1", &request.model, &request).is_none());
}

#[test]
fn all_protocols_send_the_same_stable_fallback_identity_to_workbuddy() {
    let adapter = WorkbuddyChannel::from_options("workbuddy", &json!({}));
    let account = AccountSnapshot {
        uid: "account-1".into(),
        ..Default::default()
    };
    let mut conversation_ids = Vec::new();
    for decoder in [
        decode::openai::decode_chat,
        decode::responses::decode_response,
        decode::anthropic::decode_messages,
    ] {
        for extended in [false, true] {
            let mut body = json!({
                "model": "glm-5.3",
                "messages": [{"role": "user", "content": "opening"}],
                "input": [{"role": "user", "content": "opening"}],
            });
            if extended {
                for field in ["messages", "input"] {
                    body[field].as_array_mut().unwrap().extend([
                        json!({"role": "assistant", "content": "reply"}),
                        json!({"role": "user", "content": "follow-up"}),
                    ]);
                }
            }
            let mut request = decoder(&body).unwrap();
            if extended {
                request
                    .system
                    .push(Block::text("changed runtime instructions"));
            }
            let resolved = AffinityStore::resolve_request("key-1", &request);
            let upstream = adapter.build_chat_request(&account, &resolved).unwrap();
            conversation_ids.push(
                upstream
                    .headers
                    .iter()
                    .find(|(key, _)| key == "X-Conversation-ID")
                    .unwrap()
                    .1
                    .clone(),
            );
        }
    }
    assert!(conversation_ids.windows(2).all(|pair| pair[0] == pair[1]));
}
