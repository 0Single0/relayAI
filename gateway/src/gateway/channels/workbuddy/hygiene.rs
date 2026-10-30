//! 请求卫生管道：CIR → 上游 OpenAI 形状报文。
//!
//! 每一步都是纯函数，可单测、可按渠道开关。顺序固定：
//! 拼消息 → 指纹清洗 → 工具配对自愈 → reasoning 回填 → 思考注入 → 强制流式。

use serde_json::{json, Value};

use crate::gateway::protocol::cir::{Block, CanonicalRequest, Message, Role};

/// 会触发上游 WAF / 内容审核的第三方客户端特征串。
///
/// 上游的表现是 `403` 或 `code 11128`（Illegal API invocation from an
/// unapproved channel）—— 换账号也没用，必须把特征消掉。
///
/// **处理方式不是删除**：参考实现（MIT）用 A/B 测确认过，这些串被整段抹掉后
/// 请求仍然会被判异常；正确做法是**替换成语义等价但不触发匹配的写法**
/// （只改"归属声明"这类可替换的部分，保留指令本体）。
const FINGERPRINT_REWRITES: [(&str, &str); 7] = [
    // 计费头残片：整个头连同值一起去掉（它不是正文语义的一部分）。
    ("x-anthropic-billing-hdr", ""),
    // Claude Code 的自我声明：去掉 "Anthropic's official CLI ..." 这段归属，
    // 保留 "You are Claude Code" 这个型号声明（单独出现是允许的）。
    // 两种客户端写法都要覆盖；缺少 tool 的版本已通过真实上游 A/B 验证。
    (
        "You are Claude Code, Anthropic's official CLI for Claude",
        "You are Claude Code",
    ),
    (
        "You are Claude Code, Anthropic's official CLI tool for Claude",
        "You are Claude Code",
    ),
    // Codex CLI 同理：去掉 "tool, a terminal-based ..." 这段附加描述。
    (
        "You are a coding agent running in the Codex CLI tool, a terminal-based coding assistant.",
        "You are a coding agent running in the Codex CLI tool.",
    ),
    // 反馈链接含 anthropics 域名，是明确的来源指纹。
    (
        "To provide feedback, users should report the issue at https://github.com/anthropics/claude-code/issues",
        "To provide feedback, users should report the issue on the project homepage",
    ),
    // 分支说明里的特征短语。
    ("Main branch (", "Primary branch ("),
    // 上游自己的错误码字面量：出现在正文里会被当成伪装特征。
    // 参考实现在公开发布的版本里把这个改写的目标值隐去了，无从照搬；
    // 而它只是个数字码、去掉不损失任何语义（参考实现"不要整段删除"的警告
    // 针对的是身份声明这类有语义的串），所以这里选择移除。
    ("11128", ""),
];

/// 来源域名：命中即移除（域名本身没有任何语义价值）。
const FINGERPRINT_DOMAINS: [&str; 1] = ["github.com/anthropics/"];

/// DSH（DeepSeek Harness）路径声明的**连续文本**特征。
///
/// 上游的渠道校验看的是「连续文本」：这两句紧挨着出现即判定为非官方客户端。
///
/// ```text
/// Tokens prefixed with @ are paths the user explicitly referenced.
/// Relative paths resolve from the workspace root; absolute paths identify files or directories on the host.
/// ```
///
/// 实测结论（用户侧 A/B）：
///   · 单句各自 200，**连排必 400**（code 11-128）；
///   · 改第二句无效，改**第一句**才解除匹配。
///
/// 所以只在第一句里插入一个 `that`：`... are paths that the user ...`。
/// 句意不变（"被用户显式引用的路径"），但连续文本已不匹配上游的签名。
/// 参考实现按 look-ahead 写；这里用**捕获组**替代 —— `regex` crate 不支持 lookaround，
/// 用捕获组把 `the user ...` 记住再回填，效果等价。
const DSH_PATH_PREFIX: &str = "Tokens prefixed with @ are paths ";

fn dsh_path_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        // 整句锚定：只认**完整第一句**，避免误伤正文里恰好提到 "are paths" 的正常内容。
        regex::Regex::new(
            r"Tokens prefixed with @ are paths (the user explicitly referenced)",
        )
        .expect("DSH 路径句正则是编译期常量，不会失败")
    })
}

/// 注入的最小中性 system 提示词。
///
/// 上游的渠道校验会看"请求长什么样"：**没有 system 消息的请求**本身就是一个
/// 特征（官方客户端总会带一条）。所以空手起手的请求先补一条中性的，
/// 内容刻意不含任何品牌、模型名或工具声明。
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

/// 身份声明句式：`You are an AI agent powered by X` 这类"自我介绍 + 品牌"。
///
/// 上游 WAF 把它当成"非官方渠道"（参考实现 #533 / #537 / #539）：
/// ```text
/// You are an AI agent powered by DeepSeek Harness.
/// You are a coding agent powered by the deepseek-v4.1-flash model.
/// You are an AI assistant built on DeepSeek.
/// ```
/// 大小写不敏感、连续子串匹配；单独的品帬词或改语序后放行 ——
/// 所以匹配到的**整句**换成中性句，而不是只删品牌词（只删品牌词仍会被判异常）。
///
/// 正则与参考实现逐字对齐：
/// `You are an?(?:\s+AI)?(?:\s+coding)?\s+(?:agent\s+powered by|assistant\s+built on)[^\n]*?(?:\.(?=\s|$)|(?=\n|$))`
/// 身份声明句式的**前缀**（到品牌归属词为止）。
///
/// 停在这里而不是把句尾也写进正则：句尾判定要"句号后跟空白或串尾"，
/// 而 `deepseek-v4.1-flash` 这种模型名里也有 `.`，纯正则会把 `v4.` 当句尾。
/// 句尾交给 [`identity_sentence_end`] 逐字符判断，与参考实现的 look-ahead 等价。
fn identity_prefix_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(
            r"(?i)You are an?(?:\s+AI)?(?:\s+coding)?\s+(?:agent\s+powered by|assistant\s+built on)",
        )
        .expect("身份声明正则是编译期常量，不会失败")
    })
}

/// 从 `from` 开始找一句的结尾：返回**不含**结尾的字节偏移。
///
/// 与参考实现的 `(?:\.(?=\s|$)|(?=\n|$))` 等价：
/// · 句号后面是空白或串尾 → 句子结束（句号本身属于这句话，会被替换掉）；
/// · 换行 → 句子结束（换行不属于这句话，保留）；
/// · 都没有 → 到串尾。
fn identity_sentence_end(text: &str, from: usize) -> usize {
    let mut chars = text[from..].char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' {
            return from + index;
        }
        if ch == '.' {
            let next = chars.peek().map(|(_, next)| *next);
            let ends_here = match next {
                None => true,
                Some(next) => next.is_whitespace(),
            };
            if ends_here {
                // 句号本身也要吃掉（它属于这句被替换掉的内容）。
                return from + index + ch.len_utf8();
            }
        }
    }
    text.len()
}

/// 把所有身份声明整句换成中性句。
fn rewrite_identity_declarations(text: &str) -> String {
    let regex = identity_prefix_regex();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for matched in regex.find_iter(text) {
        // 已经落在上一处替换里的匹配直接跳过（避免重复处理同一句）。
        if matched.start() < cursor {
            continue;
        }
        let end = identity_sentence_end(text, matched.end());
        output.push_str(&text[cursor..matched.start()]);
        output.push_str("You are a helpful AI assistant.");
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output
}

/// OmO 框架的身份指纹：`Sisyphus-Junior - Focused executor`。
///
/// 参考实现注释确认：上游**按连续子串**匹配，且"改掉句式结构后就不再命中"。
/// 所以这里把 `- ` 结构改成括号式（语义不变，结构已不同），
/// 比只做大小写归一化更可靠 —— 后者对原样出现的串等于没改。
fn omo_junior_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"(?i)Sisyphus-Junior\s*[-–—]\s*Focused executor")
            .expect("OmO 指纹正则是编译期常量，不会失败")
    })
}

/// 首句之外残留的品牌词：`DeepSeek Harness`（含 `deepseek-harness` / `deepseek_harness`
/// / 连续空白等变体）。
///
/// 参考实现 #562 的实测：DSH 0.2.1-rc.2 的 developer 提示里，除了首句
/// `You are an AI agent powered by DeepSeek Harness.`（身份句正则已能洗）之外，
/// 还有两句**不带 `powered by` 句式**的品牌表述：
///
/// ```text
/// The runtime context names the DeepSeek Harness implementation checkout.
/// You are interacting with the user through the DeepSeek Harness Web GUI ...
/// ```
///
/// 这两句匹配不到身份句正则，却照样稳定触发 `11-128`
/// （A/B：只留其一必拦，全部中性化成 `the runtime` 即 200）。
///
/// 作用域与身份句一致：**只在 system / developer 内生效**。user / assistant / tool
/// 是用户数据，改了会让模型看到与磁盘不一致的内容（#537 因此收窄）。
fn brand_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        // 连前面的冠词一起吃掉：替换值是 `the runtime`，否则会留下
        // "names the the runtime implementation checkout" 这种叠词。
        // `the` 前面必须是词首：否则 `breathe DeepSeek Harness` 会被吞成 `breat`。
        regex::Regex::new(r"(?i)(?:\bthe[ \t]+)?deepseek[ \t_-]*harness")
            .expect("品牌词正则是编译期常量，不会失败")
    })
}

/// 品牌词的中性替换值：保留"这是个运行时"的语义，去掉品牌归属。
const BRAND_NEUTRAL: &str = "the runtime";

/// 计费头残片的 KV 串：`cc_xxx=yyy;`。
fn cc_kv_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"(?i)\bcc_[a-z0-9_]+=[^;\r\n]*;?\s*")
            .expect("cc_* KV 正则是编译期常量，不会失败")
    })
}

fn billing_header_regex() -> &'static regex::Regex {
    static CELL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        regex::Regex::new(r"(?i)x?-anthropic-billing-header:[^;\r\n]*;?[ \t]*")
            .expect("billing header regex")
    })
}

/// 拼出上游请求体：以原始请求体为基底（保留渠道特有的透传字段），再逐层清洗。
pub fn build_chat_body(model: &str, request: &CanonicalRequest) -> Value {
    let mut body = match request.raw.clone() {
        Value::Object(map) => Value::Object(map),
        _ => json!({}),
    };
    if let Some(map) = body.as_object_mut() {
        // 私有标记键（下划线开头）是网关内部用的，不该发给上游。
        map.retain(|key, _| !key.starts_with('_'));
        map.remove("max_completion_tokens");
        // **客户端协议的顶层提示词字段必须删掉**：Anthropic 的 `system`、
        // Responses 的 `instructions` / `input` 是各自协议的顶层字段，
        // 而我们已经把提示词重建进 `messages`（且**经过指纹清洗**）。
        // 留着它们等于把客户端的原文（未清洗）再发一遍 ——
        // 既让上游看到两份提示词，也把清洗直接绕过。
        map.remove("system");
        map.remove("instructions");
        map.remove("input");
    }

    // 工具选择**归一成上游要的字符串**：上游是 Go 服务，`Request.tool_choice`
    // 声明为 string —— 发对象会 11101 "cannot unmarshal object into ... string"
    //（畸形请求有时还会撞上渠道校验的 11128）。Claude Code 指定工具时发的
    // `{type:"tool",name}` 解码出来是 OpenAI 的 function 对象，必须在这里降级；
    // 参考实现同样在最后一道做这个归一（`normalize_tool_choice`）。
    // 语义：auto/required/none 原样；指定工具 = 工具名（上游的 string 形状）。
    if let Some(choice) = &request.tool_choice {
        if let Some(normalized) = normalize_tool_choice(choice) {
            body["tool_choice"] = json!(normalized);
        }
    }

    body["model"] = json!(model);
    let mut messages = build_messages(request);
    repair_tool_pairing(&mut messages);
    backfill_reasoning_content(&mut messages, model);
    body["messages"] = Value::Array(messages);

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
                    function["description"] = json!(sanitize_text(description));
                }
                if let Some(parameters) = &tool.parameters {
                    function["parameters"] = parameters.clone();
                }
                json!({ "type": "function", "function": function })
            })
            .collect::<Vec<_>>());
    }

    match request.max_tokens {
        Some(max_tokens) => body["max_tokens"] = json!(max_tokens),
        None => {
            if let Some(map) = body.as_object_mut() {
                map.remove("max_tokens");
            }
        }
    }

    inject_deepseek_thinking(&mut body, model);
    force_stream(&mut body);
    body
}

fn build_messages(request: &CanonicalRequest) -> Vec<Value> {
    let mut messages = Vec::new();

    let system: Vec<&str> = request.system.iter().filter_map(Block::as_text).collect();
    if !system.is_empty() {
        // system 是"客户端自我声明"，允许做身份改写（user 内容不允许）。
        messages.push(json!({
            "role": "system",
            "content": sanitize_text_with(&system.join("\n"), true),
        }));
    }

    for message in &request.messages {
        messages.push(build_message(message));
    }

    // **保证请求一定以 system 开头**。
    //
    // 官方客户端总会带一条 system；"没有 system 消息"本身就是一个渠道特征。
    // 清洗后 system 变空（整段都是指纹）时也走这里补一条中性的，
    // 避免发出一条 `content: ""` 的怪 system。
    let needs_prompt = match messages.first() {
        None => true,
        Some(first) => {
            first.get("role").and_then(Value::as_str) != Some("system")
                || first
                    .get("content")
                    .and_then(Value::as_str)
                    .is_none_or(|text| text.trim().is_empty())
        }
    };
    if needs_prompt {
        // 已经有空的 system 就替换，没有才插入 —— 不能留两条 system。
        match messages.first().map(|first| first.get("role").and_then(Value::as_str)) {
            Some(Some("system")) => {
                messages[0] = json!({ "role": "system", "content": DEFAULT_SYSTEM_PROMPT });
            }
            _ => messages.insert(
                0,
                json!({ "role": "system", "content": DEFAULT_SYSTEM_PROMPT }),
            ),
        }
    }
    messages
}

fn build_message(message: &Message) -> Value {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    let mut value = json!({ "role": role, "content": content_value(&message.content) });

    if !message.tool_calls.is_empty() {
        value["tool_calls"] = json!(message
            .tool_calls
            .iter()
            .map(|call| json!({
                "id": call.id.clone().unwrap_or_default(),
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": sanitize_text(&call.arguments),
                },
            }))
            .collect::<Vec<_>>());
    }
    if let Some(id) = &message.tool_call_id {
        value["tool_call_id"] = json!(id);
    }
    if let Some(name) = &message.name {
        value["name"] = json!(name);
    }
    if let Some(reasoning) = &message.reasoning {
        // 历史思考内容同样要清：它会被整段回传给上游。
        value["reasoning_content"] = json!(sanitize_text(reasoning));
    }
    value
}

/// 有图就发块数组，纯文本发字符串（上游对两种写法都收，但纯文本更省）。
fn content_value(blocks: &[Block]) -> Value {
    let texts: Vec<&str> = blocks.iter().filter_map(Block::as_text).collect();
    let images: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Image { url } => Some(url.as_str()),
            Block::Text { .. } => None,
        })
        .collect();

    if images.is_empty() {
        return Value::String(sanitize_text(&texts.join("\n")));
    }
    let mut parts = Vec::new();
    for text in texts {
        parts.push(json!({ "type": "text", "text": sanitize_text(text) }));
    }
    for url in images {
        parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
    }
    Value::Array(parts)
}

/// 把会被判「未授权渠道」的特征串改写成等价写法，并抹平 `\r\n` 残片。
///
/// `identity` 控制是否处理**身份声明句式**（`You are an AI agent powered by X`）：
/// · `true` —— system / developer 角色。这类文本是"客户端自我声明"，
///   改写它不影响模型行为，却能去掉 WAF 最大的一个指纹；
/// · `false` —— user / assistant / tool 角色。这些是**用户数据**，
///   改写会让模型看到与磁盘不一致的内容（参考实现 #537 因此明确收窄）。
pub fn sanitize_text_with(text: &str, identity: bool) -> String {
    // 绝大多数请求没有任何特征串：先判断，省掉下面这一串无谓的拷贝与扫描。
    if !has_fingerprint_with(text, identity) {
        return text.to_string();
    }
    let mut output = text.to_string();

    // ① 身份声明整句换中性写法（最先做：它吃掉整句后，后面的规则少扫一遍）。
    if identity {
        output = rewrite_identity_declarations(&output);
        // ② 首句之外残留的品牌词（#562）：不带 `powered by` 句式，身份句正则认不出。
        // 中性化而不是删除 —— 参考实现的 A/B 结论是整段抹掉反而更像异常客户端。
        output = brand_regex()
            .replace_all(&output, BRAND_NEUTRAL)
            .into_owned();
        /*
          ②' DSH 路径声明：在 `are paths` 后插入 `that`。

          上游按**连续文本**判渠道签名，这两句连排即 400（code 11-128）。
          实测"改第二句无效、改第一句才解除匹配"，所以只动第一句。

          插入而不是删除：整段抹掉会让 system 少掉一条正常的能力说明，
          反而更像非常规客户端（与品牌词那条同一个道理）。
        */
        output = dsh_path_regex()
            .replace_all(&output, "Tokens prefixed with @ are paths that $1")
            .into_owned();
    }
    // ③ OmO 框架指纹：改句式结构而不是删词（删词不解除匹配）。
    output = omo_junior_regex()
        .replace_all(&output, "Sisyphus-Junior (focused executor)")
        .into_owned();

    output = billing_header_regex().replace_all(&output, "").into_owned();

    // 只删计费 KV，保留同一行后面的指令及正常的 cc_* 标识符。
    if output.to_ascii_lowercase().contains("cc_") {
        output = cc_kv_regex().replace_all(&output, "").into_owned();
    }
    for (from, to) in FINGERPRINT_REWRITES {
        if output.contains(from) {
            output = output.replace(from, to);
        }
    }
    for domain in FINGERPRINT_DOMAINS {
        if output.contains(domain) {
            output = output.replace(domain, "");
        }
    }
    // `\r\n` 残片会被上游当成"伪造头"的特征。
    output = output.replace("\r\n", "\n");
    // 首尾空白也带着"拼接进来的头部"形状，一并去掉（与参考实现一致）。
    output.trim().to_string()
}

/// 兼容入口：按「非身份文本」处理（user / tool 内容）。
pub fn sanitize_text(text: &str) -> String {
    sanitize_text_with(text, false)
}

/// 按角色决定要不要做身份改写：只有 system / developer 是"客户端自我声明"。
pub fn sanitize_for_role(text: &str, role: &str) -> String {
    let identity = matches!(role, "system" | "developer");
    sanitize_text_with(text, identity)
}

/// 是否含需要处理的特征（给调用方做"要不要改写"的判断，避免无谓的字符串拷贝）。
pub fn has_fingerprint(text: &str) -> bool {
    has_fingerprint_with(text, true)
}

/// `identity` = 是否把身份声明句式也算作特征（只对 system / developer 为 true）。
pub fn has_fingerprint_with(text: &str, identity: bool) -> bool {
    FINGERPRINT_REWRITES
        .iter()
        .any(|(from, _)| !from.is_empty() && text.contains(from))
        || FINGERPRINT_DOMAINS.iter().any(|domain| text.contains(domain))
        // 计费头残片：cc_* KV（WAF 指纹，见 sanitize_text 里的注释）。
        || text.contains("cc_")
        || billing_header_regex().is_match(text)
        || text.contains("\r\n")
        // OmO 框架指纹与身份声明：正则匹配（字面量认不出各种变体）。
        || omo_junior_regex().is_match(text)
        || (identity && identity_prefix_regex().is_match(text))
        // 残留品牌词（#562）：只含 `DeepSeek Harness` 而没有身份句式的文本
        // 也必须进清洗分支 —— 否则这里的提前 return 会让它原样上行。
        || (identity && brand_regex().is_match(text))
        // DSH 路径声明（上游按"连续文本"判渠道）：
        // 先做**字面量**快速判断，命中才开始跑正则 —— 绝大多数请求不带这句，
        // `contains` 比正则快一个数量级，不该为它付正则的代价。
        || (text.contains(DSH_PATH_PREFIX) && dsh_path_regex().is_match(text))
}

/// 工具配对自愈：合并相邻 assistant(tool_calls) → 按 call 重排 tool 结果 → 清孤儿。
///
/// 上游对「工具调用序列断裂」会直接 400（code 11148），所以本地先修好再发。
fn repair_tool_pairing(messages: &mut Vec<Value>) {
    merge_adjacent_tool_calls(messages);

    // 结果索引：同一 tool_call_id 只认第一条。
    let mut results: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for message in messages.iter() {
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            if let Some(id) = message.get("tool_call_id").and_then(Value::as_str) {
                results
                    .entry(id.to_string())
                    .or_insert_with(|| message.clone());
            }
        }
    }

    let mut output = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        let role = message.get("role").and_then(Value::as_str).unwrap_or_default();
        if role == "tool" {
            // tool 消息统一由它对应的 assistant 重新插入，这里跳过（清孤儿）。
            continue;
        }
        if role == "assistant" {
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array).cloned() {
                let kept: Vec<Value> = calls
                    .into_iter()
                    .filter(|call| {
                        call.get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| results.contains_key(id))
                    })
                    .collect();
                if kept.is_empty() {
                    let mut without_calls = message.clone();
                    if let Some(map) = without_calls.as_object_mut() {
                        map.remove("tool_calls");
                    }
                    output.push(without_calls);
                } else {
                    let mut with_calls = message.clone();
                    with_calls["tool_calls"] = Value::Array(kept.clone());
                    output.push(with_calls);
                    for call in kept {
                        if let Some(id) = call.get("id").and_then(Value::as_str) {
                            if let Some(result) = results.get(id) {
                                output.push(result.clone());
                            }
                        }
                    }
                }
                continue;
            }
        }
        output.push(message);
    }
    *messages = output;
}

fn merge_adjacent_tool_calls(messages: &mut Vec<Value>) {
    let mut output: Vec<Value> = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        let mergeable = message.get("role").and_then(Value::as_str) == Some("assistant")
            && message.get("tool_calls").is_some();
        if mergeable {
            if let Some(last) = output.last_mut() {
                let last_mergeable = last.get("role").and_then(Value::as_str) == Some("assistant")
                    && last.get("tool_calls").is_some();
                if last_mergeable {
                    let mut calls = last
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    calls.extend(
                        message
                            .get("tool_calls")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    );
                    last["tool_calls"] = Value::Array(calls);
                    let extra = message
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !extra.is_empty() {
                        let head = last
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        last["content"] = Value::String(format!("{head}{extra}"));
                    }
                    continue;
                }
            }
        }
        output.push(message);
    }
    *messages = output;
}

/// deepseek 系开启思考时，历史 assistant 消息缺 `reasoning_content` 会被上游拒。
fn backfill_reasoning_content(messages: &mut [Value], model: &str) {
    if !is_deepseek(model) {
        return;
    }
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && message.get("reasoning_content").is_none()
        {
            message["reasoning_content"] = json!("");
        }
    }
}

/// 思考开关是**两段式**：只发 `thinking.type=enabled` 不给 `reasoning_effort`，
/// 实测上游 reasoning_tokens 为 0（等于没开）。所以两个一起给。
fn inject_deepseek_thinking(body: &mut Value, model: &str) {
    if !is_deepseek(model) {
        return;
    }
    let opted_out = body
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("disabled"));
    let effort = body
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::to_string);
    if opted_out || effort.as_deref().is_some_and(|value| value.eq_ignore_ascii_case("none")) {
        return;
    }
    body["thinking"] = json!({ "type": "enabled" });
    if effort.is_none() {
        body["reasoning_effort"] = json!("high");
    }
}

fn is_deepseek(model: &str) -> bool {
    model.to_ascii_lowercase().starts_with("deepseek")
}

/// 把 OpenAI 形状的 `tool_choice` 归一成上游的字符串形状。
///
/// 返回 `None` = 无法识别的形状，整个字段不要发（发畸形值必被上游拒绝）。
/// `"none"` 保留 tools 声明：语义由 tool_choice 本身表达，删 tools 会让
/// 模型拿不到函数签名、把调用降级成正文里的伪 JSON（参考实现踩过的坑）。
fn normalize_tool_choice(choice: &Value) -> Option<String> {
    match choice {
        Value::String(text) => {
            let lowered = text.trim().to_ascii_lowercase();
            match lowered.as_str() {
                "auto" | "required" | "none" => Some(lowered),
                // OpenAI 的 {type:"function",function:{name}} 之外没有别的对象，
                // 字符串只可能是这三种语义；其他字符串不猜，丢掉。
                _ => None,
            }
        }
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str)?;
            match kind {
                "auto" | "required" | "none" => Some(kind.to_string()),
                "function" => Some(
                    map.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .filter(|name| !name.trim().is_empty())
                        .map(|name| name.trim().to_string())?,
                ),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 上游只支持流式：非流式由网关本地折叠；usage 只能从 `stream_options` 拿到。
fn force_stream(body: &mut Value) {
    body["stream"] = json!(true);
    let has_options = body.get("stream_options").is_some_and(Value::is_object);
    if !has_options {
        body["stream_options"] = json!({ "include_usage": true });
    }
}

#[cfg(test)]
mod tests;
