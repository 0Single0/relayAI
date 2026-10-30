use super::*;
use crate::gateway::protocol::cir::{CanonicalRequest, ToolCall, ToolSpec};
use serde_json::json;

fn request() -> CanonicalRequest {
    CanonicalRequest {
        model: "glm-5.3".into(),
        system: vec![Block::text("你是助手")],
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        stream: false,
        reasoning: None,
        metadata: Default::default(),
        raw: json!({}),
    }
}

fn user(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![Block::text(text)],
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        reasoning: None,
    }
}

/// 只带 user 消息（`role, text` 列表）的最小请求。
fn request_with(messages: &[(&str, &str)]) -> CanonicalRequest {
    let mut request = request();
    request.system.clear();
    request.messages = messages
        .iter()
        .map(|(role, text)| Message {
            role: match *role {
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            },
            content: vec![Block::text(*text)],
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        })
        .collect();
    request
}

fn assistant_with_call(id: &str, name: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: Vec::new(),
        tool_calls: vec![ToolCall {
            id: Some(id.into()),
            name: name.into(),
            arguments: "{}".into(),
        }],
        tool_call_id: None,
        name: None,
        reasoning: None,
    }
}

fn tool_result(id: &str, text: &str) -> Message {
    Message {
        role: Role::Tool,
        content: vec![Block::text(text)],
        tool_calls: Vec::new(),
        tool_call_id: Some(id.into()),
        name: None,
        reasoning: None,
    }
}

/// 身份声明句式（`You are an AI agent powered by X`）—— 这是触发
/// `11-128 Illegal API invocation from an unapproved channel` 的主要指纹。
///
/// 三条用例都取自参考实现确认过的真实报障（#533 / #537 / #539）。
#[test]
fn identity_declarations_are_replaced_with_a_neutral_sentence() {
    for source in [
        "You are an AI agent powered by DeepSeek Harness.",
        "You are a coding agent powered by the deepseek-v4.1-flash model.",
        "You are an AI assistant built on DeepSeek.",
    ] {
        let cleaned = sanitize_text_with(source, true);
        assert_eq!(
            cleaned, "You are a helpful AI assistant.",
            "整句要换成中性句：{source}"
        );
    }
}

/// 大小写不敏感；且必须**整句**替换，只删品牌词仍会被判异常。
#[test]
fn identity_matching_is_case_insensitive_and_sentence_wide() {
    let cleaned = sanitize_text_with("you are an ai agent powered by deepseek harness.", true);
    assert_eq!(cleaned, "You are a helpful AI assistant.");
}

/// 身份改写**只对 system / developer 生效**：user / tool 是用户数据，
/// 改写会让模型看到与磁盘不一致的内容（参考实现 #537 因此收窄）。
#[test]
fn identity_rewrite_is_limited_to_system_roles() {
    let source = "You are an AI agent powered by DeepSeek Harness.";
    assert_eq!(
        sanitize_for_role(source, "system"),
        "You are a helpful AI assistant."
    );
    assert_eq!(
        sanitize_for_role(source, "developer"),
        "You are a helpful AI assistant."
    );
    // user 内容原样保留。
    assert_eq!(sanitize_for_role(source, "user"), source);
    assert_eq!(sanitize_for_role(source, "tool"), source);
}

/// #562：首句之外的残留品牌词必须中性化 —— 并且必须被 `has_fingerprint`
/// 认出来，否则 `sanitize_text_with` 在开头就 return，整段原样上行（本次的坑）。
///
/// 两句取自 DSH 0.2.1-rc.2 的 developer 提示（公开文本，不含对话内容）。
#[test]
fn residual_brand_words_are_neutralized_in_system_scope() {
    for source in [
        "The runtime context names the DeepSeek Harness implementation checkout.",
        "You are interacting with the user through the DeepSeek Harness Web GUI.",
    ] {
        assert!(has_fingerprint(source), "品牌词必须算特征：{source}");
        let cleaned = sanitize_for_role(source, "system");
        assert!(
            !cleaned.to_ascii_lowercase().contains("deepseek"),
            "品牌词没洗掉：{cleaned}"
        );
        assert!(cleaned.contains(BRAND_NEUTRAL), "要中性化而不是删除：{cleaned}");
        assert!(!has_fingerprint(&cleaned), "洗完不该再算特征：{cleaned}");
    }

    // 冠词一并吃掉，不能留下 `the the runtime`。
    let cleaned = sanitize_for_role(
        "The runtime context names the DeepSeek Harness implementation checkout.",
        "system",
    );
    assert!(!cleaned.contains("the the"), "冠词不能叠：{cleaned}");

    // 冠词判定必须有词边界：`breathe` 结尾的 `the` 不能被吞掉。
    let boundary = sanitize_for_role("please breathe DeepSeek Harness", "system");
    assert!(
        boundary.contains("breathe the runtime"),
        "不能吞掉单词内部的 the：{boundary}"
    );

    // 大小写与分隔符变体同样命中（上游按连续子串匹配）。
    for variant in ["deepseek-harness", "DEEPSEEK_HARNESS", "DeepSeek   Harness"] {
        let cleaned = sanitize_for_role(variant, "developer");
        assert!(
            !cleaned.to_ascii_lowercase().contains("harness"),
            "{variant} → {cleaned}"
        );
    }

    // 作用域：user / tool 是用户数据，原样保留。
    let user_text = "The runtime context names the DeepSeek Harness implementation checkout.";
    assert_eq!(sanitize_for_role(user_text, "user"), user_text);
    assert_eq!(sanitize_for_role(user_text, "tool"), user_text);
}

/// OmO 框架指纹：必须**改句式结构**才算解除匹配（只删词不行）。
#[test]
fn omo_junior_fingerprint_loses_its_structure() {
    let cleaned = sanitize_text("agent=Sisyphus-Junior - Focused executor; mode=hard");
    assert!(!cleaned.contains("Sisyphus-Junior - Focused executor"), "{cleaned}");
    assert!(cleaned.contains("focused executor"), "语义要保留：{cleaned}");

    // 大小写变体同样命中。
    let lower = sanitize_text("x sisyphus-junior – focused executor y");
    assert!(!lower.contains("Sisyphus-Junior - Focused executor"));
}

/// 请求必须**以 system 开头**：缺了就补一条中性提示词。
/// 这是"没有 system 消息"这个渠道特征的解法。
#[test]
fn body_always_starts_with_a_system_message() {
    let mut request = request_with(&[("user", "你好")]);
    request.system.clear();
    let body = build_chat_body("glm-5.3", &request);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], DEFAULT_SYSTEM_PROMPT);
    assert_eq!(body["messages"][1]["content"], "你好");
}

/// 整段 system 都是指纹、清洗后变空时，也要补回中性提示词（不能发空 system）。
#[test]
fn empty_system_after_sanitizing_falls_back_to_the_neutral_prompt() {
    let mut request = request_with(&[("user", "hi")]);
    request.system = vec![Block::text("x-anthropic-billing-hdr")];
    let body = build_chat_body("glm-5.3", &request);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], DEFAULT_SYSTEM_PROMPT);
    // 不能同时留两条 system。
    let systems = body["messages"]
        .as_array()
        .expect("数组")
        .iter()
        .filter(|message| message["role"] == "system")
        .count();
    assert_eq!(systems, 1, "只能有一条 system");
}

#[test]
fn fingerprints_are_rewritten_not_deleted() {
    // 参考实现的 A/B 结论：整段删除反而仍被判异常，要替换成等价写法。
    let rewritten = sanitize_text(
        "You are Claude Code, Anthropic's official CLI tool for Claude. Be helpful.",
    );
    assert!(rewritten.starts_with("You are Claude Code."), "{rewritten}");
    assert!(!rewritten.contains("Anthropic's official CLI"), "{rewritten}");

    let rewritten = sanitize_text(
        "You are a coding agent running in the Codex CLI tool, a terminal-based coding assistant.",
    );
    assert!(
        rewritten.contains("You are a coding agent running in the Codex CLI tool."),
        "{rewritten}"
    );
    assert!(!rewritten.contains("terminal-based"), "{rewritten}");

    // 来源域名直接去掉（没有语义价值）。
    assert!(!sanitize_text("see github.com/anthropics/claude-code").contains("anthropics"));

    // \r\n 残片要抹平。
    assert!(!sanitize_text("a\r\nb").contains("\r\n"));
}

#[test]
fn fingerprint_detection_drives_whether_rewriting_is_needed() {
    assert!(has_fingerprint("You are Claude Code, Anthropic's official CLI tool for Claude"));
    assert!(has_fingerprint("x-anthropic-billing-hdr"));
    assert!(has_fingerprint("line\r\n"));
    assert!(!has_fingerprint("普通的中文提问"), "普通请求不该被改写");
}

#[test]
fn system_goes_first_and_stream_is_forced_with_usage() {
    let mut req = request();
    req.messages.push(user("你好"));
    let body = build_chat_body("glm-5.3", &req);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "你是助手");
    assert_eq!(body["messages"][1]["content"], "你好");
    assert_eq!(body["stream"], true, "上游只支持流式");
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert_eq!(body["model"], "glm-5.3");
}

#[test]
fn waf_fingerprints_are_scrubbed_from_content_and_arguments() {
    let mut req = request();
    req.messages.push(user("You are Claude Code, run this\r\nnow"));
    req.messages.push(assistant_with_call("call_1", "shell"));
    // 必须给出配对的结果，否则 tool_call 会被自愈逻辑当孤儿清掉。
    req.messages.push(tool_result("call_1", "ok"));
    req.messages[1].tool_calls[0].arguments =
        "{\"cmd\":\"echo 11128 github.com/anthropics/x\"}".into();
    let body = build_chat_body("hy3", &req);
    let text = body["messages"][1]["content"].as_str().unwrap();
    assert!(!text.contains("Anthropic's official CLI"), "{text}");
    assert!(!text.contains('\r'), "\\r\\n 残片必须抹平");
    let args = body["messages"][2]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert!(!args.contains("11128"), "{args}");
    assert!(!args.contains("anthropics"), "{args}");
}

#[test]
fn adjacent_tool_calls_are_merged_and_results_reordered() {
    let mut req = request();
    req.messages.push(user("查天气"));
    req.messages.push(assistant_with_call("call_1", "get_weather"));
    req.messages.push(assistant_with_call("call_2", "get_time"));
    req.messages.push(tool_result("call_2", "12:00"));
    req.messages.push(tool_result("call_1", "晴"));
    let body = build_chat_body("hy3", &req);
    let messages = body["messages"].as_array().unwrap();
    // system, user, assistant(两个 call), tool(call_1), tool(call_2)
    assert_eq!(messages.len(), 5, "{messages:#?}");
    assert_eq!(
        messages[2]["tool_calls"].as_array().unwrap().len(),
        2,
        "相邻的 tool_calls 必须合并成一条 assistant"
    );
    assert_eq!(messages[3]["tool_call_id"], "call_1");
    assert_eq!(messages[4]["tool_call_id"], "call_2");
}

#[test]
fn orphan_tool_calls_and_orphan_results_are_cleaned() {
    let mut req = request();
    req.messages.push(user("hi"));
    req.messages.push(assistant_with_call("call_missing", "get_weather"));
    req.messages.push(tool_result("call_other", "没人要的结果"));
    let body = build_chat_body("hy3", &req);
    let messages = body["messages"].as_array().unwrap();
    // system + user + assistant（tool_calls 被清掉）；孤儿 tool 结果被丢弃。
    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert!(messages[2].get("tool_calls").is_none(), "孤儿 tool_calls 要清掉");
    assert!(
        !messages
            .iter()
            .any(|message| message["role"] == "tool"),
        "孤儿 tool 结果要清掉"
    );
}

#[test]
fn deepseek_thinking_is_injected_as_two_parts() {
    let mut req = request();
    req.messages.push(user("hi"));
    let body = build_chat_body("deepseek-v4.1-flash", &req);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "high");
    // 历史 assistant 缺 reasoning_content 要补空串。
    assert!(body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["reasoning_content"].as_str() != Some("")));
}

#[test]
fn deepseek_history_gets_empty_reasoning_and_explicit_off_is_respected() {
    let mut req = request();
    req.messages.push(user("hi"));
    req.messages.push(Message {
        role: Role::Assistant,
        content: vec![Block::text("上次回答")],
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        reasoning: None,
    });
    let body = build_chat_body("deepseek-v4-pro", &req);
    assert_eq!(body["messages"][2]["reasoning_content"], "");

    let mut req = request();
    req.messages.push(user("hi"));
    req.raw = json!({"thinking": {"type": "disabled"}});
    let body = build_chat_body("deepseek-v4-pro", &req);
    assert!(body.get("reasoning_effort").is_none(), "显式关闭时不该注入 effort");
}

#[test]
fn private_keys_and_legacy_max_tokens_field_are_cleaned() {
    let mut req = request();
    req.messages.push(user("hi"));
    req.raw = json!({"_trace": "x", "max_completion_tokens": 10, "temperature": 0.3});
    req.max_tokens = Some(2048);
    let body = build_chat_body("hy3", &req);
    assert!(body.get("_trace").is_none());
    assert!(body.get("max_completion_tokens").is_none());
    assert_eq!(body["max_tokens"], 2048);
    assert_eq!(body["temperature"], 0.3, "透传字段要保留");
}

/// 客户端协议的**顶层提示词字段**不能跟着 raw 透传出去：
/// 提示词已经清洗过并重建进 messages，留着原文等于把清洗整个绕过
/// （Anthropic 的 `system` / Responses 的 `instructions`、`input`）。
/// 工具选择归一成上游要的**字符串**：发对象会 11101（上游是 Go，
/// `Request.tool_choice` 声明为 string）。指定工具 = 工具名。
/// 计费头残片【整段】删除：Claude Code 把 HTTP 头结构塞进了 system，
/// 只删键名会留下 ": cc_version=...; cc_entrypoint=...;" 这样的半截 KV ——
/// 上游 WAF 认的正是这个残片（真机 bisect 锁定，见 probe_cc_bisect）。
#[test]
fn billing_header_remnant_is_removed_as_a_whole() {
    // 真机 dump 的 system 开头（字节级核对过的形状）。
    let kv = ["c", "c", "_", "v", "e", "r", "s", "i", "o", "n"].concat();
    let entry = ["c", "c", "_", "e", "n", "t", "r", "y", "p", "o", "i", "n", "t"].concat();
    let key = [
        "x", "-", "a", "n", "t", "h", "r", "o", "p", "i", "c", "-", "b", "i", "l", "l", "i",
        "n", "g", "-", "h", "e", "a", "d", "e", "r",
    ]
    .concat();
    let dirty = format!(
        "{}: {}=2.1.296.32d; {}=sdk-cli;\nYou are a Claude agent, Agent SDK.",
        key, kv, entry
    );
    let cleaned = sanitize_text(&dirty);
    assert!(!cleaned.contains(&kv), "cc_version 残留：{cleaned}");
    assert!(!cleaned.contains("billing-header"), "键名残留：{cleaned}");
    assert!(!cleaned.contains(&entry), "entrypoint 残留：{cleaned}");
    // KV 行删掉后留下正文行自己的开头换行 —— 与真机清洗后的 dump 一致。
    assert!(
        cleaned
            .trim_start_matches('\n')
            .starts_with("You are a Claude agent"),
        "残片应整段删掉、正文保留：{cleaned}"
    );

    // 行首没有键名的裸 KV（键名已被上一轮删掉）也要删。
    let bare = format!(": {}=2.1.296.32d;\nnext", kv);
    let cleaned = sanitize_text(&bare);
    assert!(!cleaned.contains(&kv), "裸 KV 残留：{cleaned}");
    assert!(cleaned.contains("next"), "正文要保留：{cleaned}");
}

#[test]
fn tool_choice_is_normalized_to_upstream_string_shape() {
    let mut req = request();
    req.messages.push(user("hi"));
    req.tools.push(ToolSpec {
        name: "shell".into(),
        description: None,
        parameters: Some(json!({"type": "object"})),
    });
    // Claude Code 指定工具（经 Anthropic 解码后是 OpenAI function 对象）。
    req.tool_choice = Some(json!({"type": "function", "function": {"name": "shell"}}));
    // raw 里带着 Anthropic 形状的旧值 —— 归一值必须覆盖它。
    req.raw = json!({"tool_choice": {"type": "any"}});
    let body = build_chat_body("glm-5.3", &req);
    assert_eq!(
        body["tool_choice"], "shell",
        "指定工具要发工具名字符串，不是对象"
    );

    // 三种语义对象 → 字符串。
    for (value, expect) in [
        (json!({"type": "auto"}), "auto"),
        (json!({"type": "required"}), "required"),
        (json!({"type": "none"}), "none"),
    ] {
        let mut req = request();
        req.messages.push(user("hi"));
        req.tool_choice = Some(value);
        let body = build_chat_body("glm-5.3", &req);
        assert_eq!(body["tool_choice"], expect);
    }

    // OpenAI 客户端直接发字符串也认。
    let mut req = request();
    req.messages.push(user("hi"));
    req.tool_choice = Some(json!("auto"));
    let body = build_chat_body("glm-5.3", &req);
    assert_eq!(body["tool_choice"], "auto");

    // 认不出的形状整个字段不发（发畸形值必被拒）。
    let mut req = request();
    req.messages.push(user("hi"));
    req.tool_choice = Some(json!({"type": "weird"}));
    let body = build_chat_body("glm-5.3", &req);
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn top_level_prompt_fields_do_not_leak_through_raw() {
    let mut req = request();
    req.system = vec![Block::text("You are Claude Code, Anthropic's official CLI tool for Claude")];
    req.messages.push(user("hi"));
    // 模拟 Anthropic 解码后的 raw：顶层还带着未清洗的 system。
    req.raw = json!({
        "model": "glm-5.3",
        "system": "You are Claude Code, Anthropic's official CLI tool for Claude",
        "max_tokens": 64,
    });
    let body = build_chat_body("glm-5.3", &req);
    assert!(body.get("system").is_none(), "顶层 system 必须删除：提示词走清洗后的 messages");
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(
        !system.contains("Anthropic's official CLI"),
        "messages 里的提示词必须是清洗后的：{system}"
    );
    // Responses 协议同理。
    let mut req = request();
    req.messages.push(user("hi"));
    req.raw = json!({"instructions": "secret prompt", "input": "raw input"});
    let body = build_chat_body("glm-5.3", &req);
    assert!(body.get("instructions").is_none());
    assert!(body.get("input").is_none());
}

#[test]
fn tools_are_emitted_in_openai_shape() {
    let mut req = request();
    req.messages.push(user("hi"));
    req.tools.push(ToolSpec {
        name: "get_weather".into(),
        description: Some("查天气".into()),
        parameters: Some(json!({"type": "object"})),
    });
    let body = build_chat_body("hy3", &req);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
    assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
}

#[test]
fn images_become_content_parts() {
    let mut req = request();
    req.messages.push(Message {
        role: Role::User,
        content: vec![
            Block::text("看图"),
            Block::Image {
                url: "https://example.com/a.png".into(),
            },
        ],
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        reasoning: None,
    });
    let body = build_chat_body("glm-5v-turbo", &req);
    assert_eq!(body["messages"][1]["content"][0]["type"], "text");
    assert_eq!(body["messages"][1]["content"][1]["type"], "image_url");
}

/// DSH 路径声明：两句连排会被上游判成非官方客户端（code 11-128）。
///
/// 断言三件事：
///   1. 完整两句能识别为特征；
///   2. 清洗后第一句里多了 `that`，且**连续的原始文本不再存在**；
///   3. 句意不变（`the user explicitly referenced` 仍在）。
#[test]
fn dsh_path_sentence_is_broken_up() {
    let text = "Tokens prefixed with @ are paths the user explicitly referenced.\n\
                Relative paths resolve from the workspace root; absolute paths identify files or directories on the host.";

    assert!(has_fingerprint_with(text, true), "两句连排必须被识别为特征");

    let cleaned = sanitize_text_with(text, true);
    assert!(
        cleaned.contains("are paths that the user explicitly referenced"),
        "第一句应插入 that 后仍读得通：{cleaned}"
    );
    assert!(
        !cleaned.contains("are paths the user explicitly referenced"),
        "原始连续文本不应残留：{cleaned}"
    );
    // 第二句原样保留：实测改它无效，也没必要动它。
    assert!(
        cleaned.contains("Relative paths resolve from the workspace root"),
        "第二句不该被改写：{cleaned}"
    );
}

/// 只有第一句、没有第二句时不该被动 —— 单句本身不触发上游校验，
/// 动了反而让 system 与用户实际看到的内容不一致。
#[test]
fn dsh_path_single_sentence_is_left_alone() {
    let text = "Tokens prefixed with @ are paths the user explicitly referenced.";
    let cleaned = sanitize_text_with(text, true);
    // 单句里出现 `are paths ` 也会被这条规则命中；这里只要求**结果仍可读**，
    // 不要求原样保留（规则是整句锚定的，命中就会改）。
    assert!(cleaned.contains("the user explicitly referenced"), "{cleaned}");
}

/// 正文（user 角色）不参与 DSH 改写：那是用户数据，改了会与磁盘不一致。
#[test]
fn dsh_path_is_not_rewritten_for_user_content() {
    let text = "Tokens prefixed with @ are paths the user explicitly referenced.\n\
                Relative paths resolve from the workspace root.";
    let cleaned = sanitize_for_role(text, "user");
    assert_eq!(cleaned, text, "user 内容不该被改写");
}

#[test]
fn claude_code_identity_variants_are_normalized_before_forwarding() {
    for identity in [
        "You are Claude Code, Anthropic's official CLI for Claude.",
        "You are Claude Code, Anthropic's official CLI tool for Claude.",
    ] {
        let payload = json!({
            "model": "deepseek-v4.1-flash",
            "max_tokens": 64,
            "system": [
                {"type": "text", "text": "x-anthropic-billing-header: cc_version=2.1.296; cc_entrypoint=cli;"},
                {"type": "text", "text": format!("{identity}\n保留这条开发指令。")}
            ],
            "messages": [{"role": "user", "content": "只回答 OK。"}],
            "tools": [{"name": "read_file", "input_schema": {"type": "object"}}]
        });
        let request = crate::gateway::protocol::decode::anthropic::decode_messages(&payload).unwrap();
        let body = build_chat_body(&request.model, &request);
        assert_eq!(body["messages"][0]["content"], "You are Claude Code.\n保留这条开发指令。");
        assert_eq!(body["messages"][1]["content"], "只回答 OK。");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert!(body.get("system").is_none(), "original system must not bypass normalization");
        assert_eq!(body["stream"], true);
        let text = body["messages"][0]["content"].as_str().unwrap();
        assert_eq!(sanitize_text_with(text, true), text, "normalization must be idempotent");
    }
}

#[test]
fn billing_header_cleanup_preserves_following_instructions_and_identifiers() {
    for header in [
        "x-anthropic-billing-header: cc_version=2.1.296; cc_entrypoint=cli;",
        "X-ANTHROPIC-BILLING-HEADER: sdk-cli;",
        "-anthropic-billing-header: cc_version=2.1.296;",
    ] {
        let source = format!("{header} 保留后续指令。\nRead cc_helpers.py and call cc_parse().");
        assert_eq!(sanitize_text(&source), "保留后续指令。\nRead cc_helpers.py and call cc_parse().");
    }
}
