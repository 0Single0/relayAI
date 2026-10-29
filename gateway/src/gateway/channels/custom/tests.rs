use serde_json::json;

use crate::gateway::channels::ChannelAdapter;
use crate::gateway::protocol::cir::{Block, CanonicalEvent, CanonicalRequest, Message, Role};

use super::catalog::{configured_models, models_url_candidates, parse_model_list};
use super::decode::{parse_anthropic_event, parse_responses_event};
use super::encode::{
    build_anthropic_body, build_openai_body, build_responses_body, normalize_tool_choice,
    normalize_tool_choice_anthropic,
};
use super::{body_shape, AccountSnapshot, CustomChannel, UpstreamProtocol};

fn request() -> CanonicalRequest {
    CanonicalRequest {
        model: "gpt-x".into(),
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
                content: Vec::new(),
                tool_calls: vec![crate::gateway::protocol::cir::ToolCall {
                    id: Some("call_1".into()),
                    name: "get_weather".into(),
                    arguments: "{\"city\":\"上海\"}".into(),
                }],
                tool_call_id: None,
                name: None,
                reasoning: None,
            },
            Message {
                role: Role::Tool,
                content: vec![Block::text("晴")],
                tool_calls: Vec::new(),
                tool_call_id: Some("call_1".into()),
                name: None,
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
        raw: json!({ "temperature": 0.3, "system": "旧值", "stream": false }),
    }
}

#[tokio::test]
async fn account_model_catalogs_are_isolated() {
    let channel = CustomChannel::from_options("custom", &json!({"models": ["global-model"]}));
    let client = reqwest::Client::new();
    for name in ["account-a-model", "account-b-model"] {
        let account = AccountSnapshot { options: Some(json!({"models": [name]})), ..Default::default() };
        let models = channel.list_models(&client, &account).await.unwrap().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, name);
    }
    let empty = AccountSnapshot { options: Some(json!({"models": []})), ..Default::default() };
    assert!(channel.list_models(&client, &empty).await.unwrap().unwrap().is_empty());
}

#[tokio::test]
async fn model_discovery_uses_standard_endpoint_and_auth() {
    use axum::{http::HeaderMap, routing::get, Json, Router};
    let app = Router::new().route("/v1/models", get(|headers: HeaderMap| async move {
        assert_eq!(headers["authorization"], "Bearer test-key");
        Json(json!({"data": [{"id": "model-a"}, {"id": "model-b"}, {"id": "model-a"}]}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let result = super::fetch_models(&reqwest::Client::new(), &format!("http://{address}/v1"), None, "test-key", "openai-chat").await.unwrap();
    assert_eq!(result.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(), ["model-a", "model-b"]);
    server.abort();
}

#[test]
fn openai_body_rebuilds_messages_and_drops_client_system_field() {
    let body = build_openai_body(&request());
    assert_eq!(body["model"], "gpt-x");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "你是助手");
    assert_eq!(body["messages"][1]["content"], "你好");
    // 透传字段保留，客户端协议的顶层字段删掉（已重建进 messages）。
    assert_eq!(body["temperature"], 0.3);
    assert!(body.get("system").is_none());
    // 上游只支持流式：强制打开。
    assert_eq!(body["stream"], true);
    // tool 结果保留 call id。
    assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
}

#[test]
fn account_model_settings_drive_outbound_requests() {
    let channel = CustomChannel::from_options("custom", &json!({"models": [{"id": "gpt-x", "max_output_tokens": 10}]}));
    let mut account = AccountSnapshot {
        base_url: Some("https://example.test/v1".into()),
        options: Some(json!({"models": [{
            "id": "gpt-x", "display_name": "Configured name", "context_length": 1000000,
            "max_output_tokens": 32000, "capabilities": {"vision": true, "reasoning": true},
            "reasoning_efforts": ["off", "high"], "reasoning_effort_map": {"off": "none", "high": "max"}
        }]})),
        ..Default::default()
    };
    let metadata = channel.model_meta(&account, "gpt-x").unwrap();
    assert_eq!(metadata.display_name.as_deref(), Some("Configured name"));
    assert_eq!(metadata.context_length, Some(1000000));
    assert!(metadata.capabilities.vision);
    let mut req = request();
    req.max_tokens = None;
    req.reasoning = Some(crate::gateway::protocol::cir::Reasoning { enabled: Some(true), effort: Some("high".into()) });
    for protocol in ["openai-chat", "openai-responses", "anthropic"] {
        account.protocol = Some(protocol.into());
        let body = channel.build_chat_request(&account, &req).unwrap().body;
        match protocol {
            "openai-chat" => { assert_eq!(body["reasoning_effort"], "max"); assert_eq!(body["max_tokens"], 32000); }
            "openai-responses" => { assert_eq!(body["reasoning"]["effort"], "max"); assert_eq!(body["max_output_tokens"], 32000); }
            _ => { assert_eq!(body["thinking"]["type"], "adaptive"); assert_eq!(body["output_config"]["effort"], "max"); assert_eq!(body["max_tokens"], 32000); }
        }
        let mut disabled = req.clone();
        disabled.reasoning.as_mut().unwrap().enabled = Some(false);
        let body = channel.build_chat_request(&account, &disabled).unwrap().body;
        match protocol {
            "openai-chat" => assert_eq!(body["reasoning_effort"], "none"),
            "openai-responses" => assert_eq!(body["reasoning"]["effort"], "none"),
            _ => assert_eq!(body["thinking"]["type"], "disabled"),
        }
    }
    req.max_tokens = Some(200);
    assert_eq!(channel.build_chat_request(&account, &req).unwrap().body["max_tokens"], 200);
    account.options = Some(json!({"models": []}));
    assert!(channel.model_meta(&account, "gpt-x").is_none(), "account catalog must not inherit another catalog's settings");
}

#[test]
fn anthropic_body_merges_consecutive_user_messages() {
    let mut req = request();
    req.messages.insert(
        1,
        Message {
            role: Role::User,
            content: vec![Block::text("再来一句")],
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        },
    );
    let body = build_anthropic_body(&req, Some(8192));
    let messages = body["messages"].as_array().unwrap();
    // system 是顶层字段，不在 messages 里。
    assert_eq!(body["system"], "你是助手");
    // 前两条 user 合并成一条，且严格交替。
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"].as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "call_1");
    // 客户端没给 max_tokens 时用配置的输出上限。
    assert_eq!(body["max_tokens"], 8192);
}

#[test]
fn tool_choice_is_normalised_both_directions() {
    assert_eq!(normalize_tool_choice(&json!("auto")), Some(json!("auto")));
    assert_eq!(
        normalize_tool_choice(&json!({"type": "tool", "name": "f"})),
        Some(json!({"type": "function", "function": {"name": "f"}}))
    );
    assert_eq!(normalize_tool_choice(&json!({"type": "any"})), Some(json!("required")));
    assert_eq!(
        normalize_tool_choice_anthropic(&json!({"type": "function", "function": {"name": "f"}})),
        Some(json!({"type": "tool", "name": "f"}))
    );
    assert_eq!(
        normalize_tool_choice_anthropic(&json!("required")),
        Some(json!({"type": "any"}))
    );
}

#[test]
fn models_url_candidates_avoid_double_version_segment() {
    assert_eq!(
        models_url_candidates("https://api.example.com", None),
        vec![
            "https://api.example.com/v1/models",
            "https://api.example.com/models"
        ]
    );
    // 已带版本段：只补 /models，不再补 /v1。
    assert_eq!(
        models_url_candidates("https://api.example.com/v1", None),
        vec!["https://api.example.com/v1/models"]
    );
    assert_eq!(
        models_url_candidates("https://open.bigmodel.cn/api/coding/paas/v4", None),
        vec![
            "https://open.bigmodel.cn/api/coding/paas/v4/models",
            "https://open.bigmodel.cn/api/coding/paas/v4/v1/models"
        ]
    );
    // Anthropic 子路径：剥掉后再试根地址。
    let candidates = models_url_candidates("https://api.z.ai/api/anthropic", None);
    assert_eq!(candidates[0], "https://api.z.ai/api/anthropic/v1/models");
    assert!(candidates.contains(&"https://api.z.ai/v1/models".to_string()));
    // 完整端点：截到版本根。
    assert_eq!(
        models_url_candidates("https://api.example.com/v1/chat/completions", None),
        vec!["https://api.example.com/v1/models"]
    );
    // 手动覆盖优先且唯一。
    assert_eq!(
        models_url_candidates("https://x.com", Some("https://y.com/list")),
        vec!["https://y.com/list"]
    );
}

#[test]
fn model_list_parsing_covers_the_common_dialects() {
    // OpenAI / Anthropic。
    let openai = parse_model_list(&json!({
        "object": "list",
        "data": [
            {"id": "gpt-4o", "context_length": 128000, "max_output_tokens": 16384,
             "capabilities": {"vision": true, "tool_calls": true}},
            {"id": "text-embed"},
        ]
    }));
    assert_eq!(openai.len(), 2);
    assert_eq!(openai[0].id, "gpt-4o");
    assert_eq!(openai[0].context_length, Some(128_000));
    assert_eq!(openai[0].max_output_tokens, Some(16_384));
    assert!(openai[0].capabilities.vision && openai[0].capabilities.tool_calls);

    let anthropic = parse_model_list(&json!({
        "data": [{"id": "claude-sonnet-4", "display_name": "Sonnet",
                  "max_input_tokens": 200000, "max_tokens": 8192}],
        "has_more": false
    }));
    assert_eq!(anthropic[0].id, "claude-sonnet-4");
    assert_eq!(anthropic[0].context_length, Some(200_000));
    assert_eq!(anthropic[0].max_output_tokens, Some(8_192));

    // Google：`models/` 前缀要去掉。
    let google = parse_model_list(&json!({
        "models": [{"name": "models/gemini-2.5-pro", "inputTokenLimit": 1048576,
                    "outputTokenLimit": 65536}]
    }));
    assert_eq!(google[0].id, "gemini-2.5-pro");
    assert_eq!(google[0].context_length, Some(1_048_576));

    // Ollama / LM Studio / 智谱。
    assert_eq!(
        parse_model_list(&json!({"models": [{"name": "qwen3:8b"}]}))[0].id,
        "qwen3:8b"
    );
    assert_eq!(
        parse_model_list(&json!({"models": [{"key": "local-model"}]}))[0].id,
        "local-model"
    );
    assert_eq!(
        parse_model_list(&json!({"models": [{"slug": "glm-4.7"}]}))[0].id,
        "glm-4.7"
    );

    // Together：顶层数组。
    assert_eq!(
        parse_model_list(&json!([{"id": "together-model"}]))[0].id,
        "together-model"
    );

    // 一条坏数据不影响其余条目；空响应不 panic。
    assert_eq!(
        parse_model_list(&json!({"data": [null, 42, {"no_id": 1}, {"id": "ok"}]}))[0].id,
        "ok"
    );
    assert!(parse_model_list(&json!({"object": "list", "data": []})).is_empty());
    assert!(parse_model_list(&json!("nonsense")).is_empty());
}

#[test]
fn configured_models_accept_plain_ids_and_metadata() {
    let models = configured_models(&json!({
        "models": [
            "plain-id",
            {"id": "rich", "credits": "x0.11", "context_length": 1000000,
             "max_output_tokens": 128000, "vision": true, "tool_calls": true,
             "reasoning": true, "reasoning_efforts": ["low","high","max"],
             "reasoning_default_effort": "high", "always_reasoning": true},
            "plain-id"
        ]
    }));
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "plain-id");
    assert_eq!(models[1].credits.as_deref(), Some("x0.11"));
    assert_eq!(models[1].context_length, Some(1_000_000));
    assert_eq!(models[1].reasoning_efforts, vec!["low", "high", "max"]);
    assert!(models[1].always_reasoning);
    assert!(!models[1].from_snapshot, "配置是当前事实，不是回落快照");
}

#[test]
fn anthropic_frames_become_cir_events() {
    let start = parse_anthropic_event(
        &json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":5}}}).to_string(),
    );
    assert!(matches!(&start[0], CanonicalEvent::Usage(usage) if usage.prompt_tokens == 15 && usage.cached_tokens == 5));

    let text = parse_anthropic_event(
        &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}).to_string(),
    );
    assert!(matches!(&text[0], CanonicalEvent::Text { text } if text == "你"));

    let thinking = parse_anthropic_event(
        &json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"想"}}).to_string(),
    );
    assert!(matches!(&thinking[0], CanonicalEvent::Reasoning { text } if text == "想"));

    let tool = parse_anthropic_event(
        &json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather"}}).to_string(),
    );
    assert!(matches!(&tool[0], CanonicalEvent::ToolCall { index: 1, name: Some(name), .. } if name == "get_weather"));

    let args = parse_anthropic_event(
        &json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\""}}).to_string(),
    );
    assert!(matches!(&args[0], CanonicalEvent::ToolCall { arguments_delta: Some(delta), .. } if delta == "{\"city\""));

    let delta = parse_anthropic_event(
        &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}).to_string(),
    );
    assert!(delta.iter().any(|event| matches!(event, CanonicalEvent::Usage(usage) if usage.completion_tokens == 7)));
    assert!(delta.iter().any(|event| matches!(event, CanonicalEvent::Finish { reason } if reason == "tool_calls")));

    let error = parse_anthropic_event(
        &json!({"type":"error","error":{"type":"overloaded_error","message":"上游过载"}}).to_string(),
    );
    assert!(matches!(&error[0], CanonicalEvent::Error { message } if message == "上游过载"));

    // 未知事件与坏 JSON 都不该中断流。
    assert!(parse_anthropic_event(&json!({"type":"ping"}).to_string()).is_empty());
    assert!(parse_anthropic_event("not json").is_empty());
}

#[test]
fn chat_url_and_auth_headers_follow_the_protocol() {
    // 现在地址与协议都从**账号**取；渠道那一份是兜底。
    // 这里给一个没有账号级配置的快照，验证"渠道兜底"这条路径仍然成立。
    let no_account = AccountSnapshot::default();

    let openai = CustomChannel::from_options(
        "p1",
        &json!({"base_url": "https://api.example.com/v1", "protocol": "openai-chat"}),
    );
    assert_eq!(
        openai.chat_url(
            &openai.base_url_for(&no_account),
            openai.protocol_for(&no_account)
        ),
        "https://api.example.com/v1/chat/completions"
    );
    let headers = openai.auth_headers(&AccountSnapshot {
        access_token: Some("sk-1".into()),
        ..AccountSnapshot::default()
    });
    let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"Authorization") && names.contains(&"x-api-key"));

    let anthropic = CustomChannel::from_options(
        "p2",
        &json!({"base_url": "https://api.z.ai/api/anthropic", "protocol": "anthropic-messages"}),
    );
    assert_eq!(
        anthropic.chat_url(
            &anthropic.base_url_for(&no_account),
            anthropic.protocol_for(&no_account)
        ),
        "https://api.z.ai/api/anthropic/messages"
    );
    let headers = anthropic.auth_headers(&AccountSnapshot {
        access_token: Some("key-1".into()),
        ..AccountSnapshot::default()
    });
    let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    // Anthropic 只用 x-api-key，不能带 Authorization（会 401）。
    assert!(names.contains(&"x-api-key") && names.contains(&"anthropic-version"));
    assert!(!names.contains(&"Authorization"));

    // 完整端点不再拼接。
    let full = CustomChannel::from_options(
        "p3",
        &json!({"base_url": "https://api.example.com/v1/chat/completions"}),
    );
    assert_eq!(
        full.chat_url(&full.base_url_for(&no_account), full.protocol_for(&no_account)),
        "https://api.example.com/v1/chat/completions"
    );
}

/// 账号上的地址与协议**优先于**渠道：这是"改账号地址必须生效"的保证。
///
/// 反过来的话，用户改账号里的 Base URL 会毫无效果（被渠道值盖住），
/// 表现就是"改了没用"——最难受的一类 bug。
#[test]
fn account_base_url_and_protocol_override_the_channel() {
    let channel = CustomChannel::from_options(
        "p1",
        &json!({"base_url": "https://channel-default.example.com/v1", "protocol": "openai-chat"}),
    );

    let account = AccountSnapshot {
        base_url: Some("https://account-specific.example.com/v1".into()),
        protocol: Some("anthropic".into()),
        ..AccountSnapshot::default()
    };

    let base = channel.base_url_for(&account);
    let protocol = channel.protocol_for(&account);
    assert_eq!(base, "https://account-specific.example.com/v1");
    assert_eq!(protocol, UpstreamProtocol::Anthropic);
    assert_eq!(
        channel.chat_url(&base, protocol),
        "https://account-specific.example.com/v1/messages",
        "协议跟着账号走，端点也要跟着变"
    );
}

#[test]
fn keyless_local_provider_does_not_require_a_credential() {
    let channel = CustomChannel::from_options(
        "ollama",
        &json!({"base_url": "http://127.0.0.1:11434/v1", "require_api_key": false}),
    );
    assert!(!channel.requires_credential());
    assert!(channel.static_credential());
    assert!(channel
        .auth_headers(&AccountSnapshot::default())
        .iter()
        .all(|(name, _)| name != "Authorization"));
}

#[test]
fn responses_body_follows_the_official_shape() {
    let mut req = request();
    req.tools = vec![crate::gateway::protocol::cir::ToolSpec {
        name: "get_weather".into(),
        description: Some("查天气".into()),
        parameters: Some(json!({"type": "object", "properties": {}})),
    }];
    req.max_tokens = Some(1024);
    req.raw = json!({ "temperature": 0.5, "messages": "残留", "stream_options": {} });
    let body = build_responses_body(&req);

    // Responses 没有 messages/system：system 进 instructions，消息进 input。
    assert_eq!(body["instructions"], "你是助手");
    assert!(body.get("messages").is_none());
    assert!(body.get("system").is_none());
    assert_eq!(body["model"], "gpt-x");
    assert_eq!(body["max_output_tokens"], 1024, "官方字段是 max_output_tokens");
    assert!(body.get("max_tokens").is_none());
    // 采样参数透传；客户端协议残留字段被剥掉。
    assert_eq!(body["temperature"], 0.5);
    assert!(body.get("stream_options").is_none());
    // 官方 schema 把 strict 列为必填，且工具是扁平形状（不套 function）。
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "get_weather");
    assert_eq!(body["tools"][0]["strict"], false);
    assert!(body["tools"][0].get("function").is_none());
    // 工具在场上时 tool_choice 有默认值；网关无状态关 store。
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);

    // input items：user 消息保角色；assistant 的 tool_call 是独立 item；
    // tool 结果是 function_call_output。
    let items = body["input"].as_array().unwrap();
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["content"][0]["type"], "input_text");
    assert_eq!(items[1]["type"], "function_call");
    assert_eq!(items[1]["call_id"], "call_1");
    assert_eq!(items[2]["type"], "function_call_output");
    assert_eq!(items[2]["call_id"], "call_1");
}

#[test]
fn responses_frames_become_cir_events() {
    let text = parse_responses_event(
        &json!({"type":"response.output_text.delta","delta":"你"}).to_string(),
    );
    assert!(matches!(&text[0], CanonicalEvent::Text { text } if text == "你"));

    let reasoning = parse_responses_event(
        &json!({"type":"response.reasoning_summary_text.delta","delta":"想"}).to_string(),
    );
    assert!(matches!(&reasoning[0], CanonicalEvent::Reasoning { text } if text == "想"));

    let call = parse_responses_event(
        &json!({"type":"response.output_item.added","output_index":1,
                "item":{"type":"function_call","call_id":"call_9","name":"get_weather"}})
            .to_string(),
    );
    assert!(matches!(&call[0], CanonicalEvent::ToolCall { index: 1, id: Some(id), name: Some(name), .. }
        if id == "call_9" && name == "get_weather"));

    let args = parse_responses_event(
        &json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"city\""}).to_string(),
    );
    assert!(matches!(&args[0], CanonicalEvent::ToolCall { arguments_delta: Some(delta), .. } if delta == "{\"city\""));

    let done = parse_responses_event(
        &json!({"type":"response.completed","response":{"status":"completed","usage":{
            "input_tokens":10,"output_tokens":7,
            "input_tokens_details":{"cached_tokens":4},
            "output_tokens_details":{"reasoning_tokens":3},
            "total_tokens":17}}}).to_string(),
    );
    assert!(done.iter().any(|event| matches!(event, CanonicalEvent::Usage(usage)
        if usage.prompt_tokens == 10 && usage.completion_tokens == 7 && usage.cached_tokens == 4)));
    assert!(done.iter().any(|event| matches!(event, CanonicalEvent::Finish { reason } if reason == "stop")));

    let incomplete = parse_responses_event(
        &json!({"type":"response.incomplete","response":{"status":"incomplete","usage":{"input_tokens":1,"output_tokens":2}}}).to_string(),
    );
    assert!(incomplete.iter().any(|event| matches!(event, CanonicalEvent::Finish { reason } if reason == "length")));

    let failed = parse_responses_event(
        &json!({"type":"response.failed","response":{"error":{"message":"过载"}}}).to_string(),
    );
    assert!(matches!(&failed[0], CanonicalEvent::Error { message } if message == "过载"));

    let error = parse_responses_event(&json!({"type":"error","message":"bad request"}).to_string());
    assert!(matches!(&error[0], CanonicalEvent::Error { message } if message == "bad request"));

    // 未知事件与坏 JSON 都不该中断流。
    assert!(parse_responses_event(&json!({"type":"response.created"}).to_string()).is_empty());
    assert!(parse_responses_event("not json").is_empty());
}

#[test]
fn responses_protocol_routes_to_the_responses_endpoint() {
    let no_account = AccountSnapshot::default();
    let channel = CustomChannel::from_options(
        "p4",
        &json!({"base_url": "https://api.example.com/v1", "protocol": "openai-responses"}),
    );
    assert_eq!(
        channel.chat_url(
            &channel.base_url_for(&no_account),
            channel.protocol_for(&no_account)
        ),
        "https://api.example.com/v1/responses"
    );
    // 鉴权与 chat 同族（Bearer + x-api-key），不是 anthropic 的 x-api-key-only。
    let headers = channel.auth_headers(&AccountSnapshot {
        access_token: Some("sk-1".into()),
        ..AccountSnapshot::default()
    });
    assert!(headers
        .iter()
        .any(|(name, _)| name == "Authorization"));

    // 完整端点不再拼接。
    let full = CustomChannel::from_options(
        "p5",
        &json!({"base_url": "https://api.example.com/v1/responses"}),
    );
    assert_eq!(
        full.chat_url(&full.base_url_for(&no_account), full.protocol_for(&no_account)),
        "https://api.example.com/v1/responses"
    );
    let _ = UpstreamProtocol::parse("responses");
}

/// 华为系上游把错误塞进 200 SSE 流里；必须识别，并附带**出站报文结构**。
///
/// 上游只说 `The request param is invalid`，不说哪个字段 ——
/// 报文又是从客户端原始 body 派生的（Responses 专有字段会被带出），
/// 所以错误里要能直接看到"我们发了哪些键"。
#[test]
fn embedded_stream_error_reports_the_outgoing_shape() {
    let channel = CustomChannel::from_options(
        "华为",
        &json!({"base_url": "https://api.modelarts-maas.com/v2", "protocol": "openai-chat"}),
    );
    // 先"发"一个带跨协议字段的报文，让渠道记住形状。
    let mut body = build_openai_body(&request());
    body["reasoning"] = json!({"effort": "medium"});
    body["include"] = json!(["reasoning.encrypted_content"]);
    if let Ok(mut slot) = channel.last_shape.lock() {
        *slot = Some(body_shape(&body));
    }

    let events = channel.parse_event(
        r#"{"text":"[DONE]","error_code":"InferHub.001001005.400","error_msg":"The request param is invalid, Please check it"}"#,
    );
    let message = events
        .iter()
        .find_map(|event| match event {
            CanonicalEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("必须转成 Error 事件：{events:?}"));
    assert!(message.contains("InferHub.001001005.400"), "{message}");
    assert!(message.contains("出站报文结构"), "{message}");
    // 跨协议字段要被点名，这是排障的关键信息。
    assert!(message.contains("跨协议字段"), "{message}");
    assert!(message.contains("reasoning"), "{message}");
    assert!(message.contains("include"), "{message}");

    // 正常帧不能被误判。
    assert!(channel
        .parse_event(r#"{"choices":[{"delta":{"content":"hi"}}]}"#)
        .iter()
        .all(|event| !matches!(event, CanonicalEvent::Error { .. })));
    // `error_code: "0"` 是正常结束。
    assert!(channel
        .parse_event(r#"{"error_code":"0","text":"[DONE]"}"#)
        .iter()
        .all(|event| !matches!(event, CanonicalEvent::Error { .. })));
}

/// 形状摘要不能泄露正文。
#[test]
fn body_shape_never_leaks_message_text() {
    let body = json!({
        "model": "deepseek-v4.1-flash",
        "stream": true,
        "messages": [
            {"role": "system", "content": "机密提示词"},
            {"role": "user", "content": "我的身份证号是 123456"}
        ],
        "tools": [{"type":"function","function":{"name":"get_weather"}}]
    });
    let shape = body_shape(&body);
    assert!(!shape.contains("机密提示词"), "{shape}");
    assert!(!shape.contains("123456"), "{shape}");
    // 但结构信息要在。
    assert!(shape.contains("keys="), "{shape}");
    assert!(shape.contains("messages=2"), "{shape}");
    assert!(shape.contains("system,user"), "{shape}");
    assert!(shape.contains("get_weather"), "{shape}");
}
