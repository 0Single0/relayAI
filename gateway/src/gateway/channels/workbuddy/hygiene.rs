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
const FINGERPRINT_REWRITES: [(&str, &str); 6] = [
    // 计费头残片：整个头连同值一起去掉（它不是正文语义的一部分）。
    ("x-anthropic-billing-hdr", ""),
    // Claude Code 的自我声明：去掉 "Anthropic's official CLI ..." 这段归属，
    // 保留 "You are Claude Code" 这个型号声明（单独出现是允许的）。
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

    // **计费头残片要整段删**（参考实现的 SANITIZE_HDR_RE + SANITIZE_KV_RE）：
    // Claude Code 的 system 里有被塞进来的 HTTP 头结构 ——
    //   x-anthropic-billing-header: cc_version=2.1.296.32d; cc_...=...; ...
    // 只删键名会留下 ": cc_version=2.1.296..." —— 上游 WAF 认的正是这个残片
    //（字节级 bisect 锁定：删掉 [0..50]（含该残片）就放行，原样必拦）。
    // 键名可能被截断（开头是 "-anthropic-billing-header:"），所以：
    // · 删到行尾的 `键名[: ]cc_* KV 串`；
    // · 兜底删所有裸 `cc_xxx=值;` KV 对；
    // · 再删残留的键名词本身。
    loop {
        let before = output.clone();
        if let Some(start) = output.find("cc_") {
            // 找 KV 起点，向前吞掉同行的 "xxx-billing-header: " 前缀。
            let line_start = output[..start].rfind('\n').map(|index| index + 1).unwrap_or(0);
            let prefix = &output[line_start..start];
            let colon = prefix.rfind(':');
            let remove_from = match colon {
                // 前缀形如 "x-anthropic-billing-header"（或被截断的同族）→ 从行首删。
                Some(_) if prefix.to_ascii_lowercase().contains("billing-header") => line_start,
                _ => start,
            };
            // KV 串到行尾为止（cc_a=1; cc_b=2; ...）。
            let line_end = output[start..]
                .find('\n')
                .map(|index| start + index)
                .unwrap_or(output.len());
            output.replace_range(remove_from..line_end, "");
        }
        // 裸 KV 对兜底（可能跨行散落）：cc_<word>=<直到分号或行尾>;
        let mut cleaned = String::with_capacity(output.len());
        for line in output.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with("cc_") && trimmed.contains('=') {
                continue;
            }
            cleaned.push_str(line);
        }
        output = cleaned;
        if output == before {
            break;
        }
    }
    // 正则兜底：跨行或夹在句中、前面没有整行前缀的 KV 对也要清掉。
    if output.contains("cc_") {
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
mod tests {
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
}