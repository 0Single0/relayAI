//! Trae 模型目录：`GET /api/remote/v1/models`。
//!
//! 上游按 **tier 分组**返回（`solo_agent_remote` / `solo_work_remote` /
//! `solo_design_remote`），同一模型可能同时出现在多组里。我们只取
//! Agent 档（能力最全、工具调用走原生协议），Work/Design 档的独有模型
//! 才追加进来 —— 同名重复会让矩阵里出现指向同一上游的两条候选。
//!
//! 两个必守规则：
//! ① 拉取失败返回 `Ok(None)`（回落内置快照），**绝不**返回空 vec ——
//!    那等于对外宣布"这个渠道没有模型"，矩阵会把这渠道的所有路由删掉；
//! ② 模型名**大小写敏感**：上游按 `config_name` 精确匹配，目录里怎么拼就怎么发。

use std::collections::HashSet;

use serde_json::Value;

use crate::gateway::{
    channels::{AccountSnapshot, ModelCapabilities, UpstreamModel},
    errors::GatewayError,
    transport,
};

use super::identity::{self, Hosts};

/// 目录里的分组：Agent 档优先，其余按顺序补齐独有模型。
const TIER_ORDER: [&str; 3] = ["solo_agent_remote", "solo_work_remote", "solo_design_remote"];

/// 内部/控制类模型：不进可选项（它们不对通用对话开放）。
const HIDDEN: [&str; 6] = [
    "auto",
    "chat_v3",
    "builder_v3",
    "solo_agent_remote",
    "solo_work_remote",
    "solo_design_remote",
];

/// 内置快照：**故意留空**。
///
/// 这里曾经硬编码过 6 个模型名当兜底，结果是有害的：
/// 它们和实时目录的**大小写拼写不一致**（实时是 `DeepSeek-V4-Pro-Official`，
/// 快照写的是 `deepseek-v4-pro`），于是矩阵里多出 6 条同名的"影子模型"，
/// 面板上重复显示、元数据全空（`?`），用户完全分不清哪个是真的。
///
/// 拉不到实时目录时的正确行为是**明说拉取失败**，让矩阵没有候选、
/// 面板显示"该渠道目录拉取失败"，而不是用一份编造的清单冒充成功。
const SNAPSHOT: [&str; 0] = [];

/// 内置快照（矩阵的 `builtin` 来源）。当前恒为空，见 [`SNAPSHOT`] 的说明。
pub fn snapshot() -> Vec<UpstreamModel> {
    SNAPSHOT
        .iter()
        .map(|id| UpstreamModel {
            id: (*id).to_string(),
            vendor: Some("trae".into()),
            from_snapshot: true,
            ..UpstreamModel::default()
        })
        .collect()
}

/// 拉实时目录。`Ok(None)` = 拉取失败，请回落快照。
///
/// **两个接口都要打**，这是元数据完整性的关键：
/// · `GET /api/remote/v1/models` —— 决定**有哪些模型**（分组、multimodal 标记），
///   但每条是 **slim object**，没有上下文长度；
/// · `POST /api/ide/v1/get_detail_param` —— 决定**每个模型怎么用**
///   （`context_window_size` / `max_tokens` / 思考档位等）。
///
/// 只打第一个接口的表现就是面板上"图片徽标有、上下文全是空"——
/// 这曾经被误判成字段名读错，实际是元数据根本不在那个响应里。
pub async fn fetch(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Result<Option<Vec<UpstreamModel>>, GatewayError> {
    // 没有凭据时不去打扰上游：直接把"失败"告诉调用方，让它用快照。
    if account.access_token.as_deref().unwrap_or_default().is_empty() {
        return Ok(None);
    }
    let payload = match transport::get_json(
        client,
        &hosts.models_url(),
        &identity::base_headers(account, "application/json"),
    )
    .await
    {
        Ok(payload) => payload,
        // 凭据失效要如实上报（它会触发刷新）；其它失败一律回落快照。
        Err(error) if error.kind == crate::gateway::errors::ErrorKind::AuthExpired => {
            return Err(error)
        }
        Err(_) => return Ok(None),
    };
    let mut models = parse_catalog(&payload);
    // 空列表按"失败"处理：上游在凭据不可用时返回 200 + 空列表。
    if models.is_empty() {
        return Ok(None);
    }

    // 只有**真的缺元数据**时才去打第二个接口。
    //
    // 实测 `/models` 已经带齐了上下文（`context_window_tokens`）、倍率
    // （`features.consumption_rate`）与思考档位（`reasoning_effort_config`），
    // 所以正常情况下完全不需要 `get_detail_param` —— 少一次网络往返，
    // 也少一条"元数据目录为空"的噪音日志。
    // 保留这条补漏路径是为了上游将来把字段移走时不至于全空。
    if models
        .iter()
        .any(|model| model.context_length.is_none() || model.reasoning_efforts.is_empty())
    {
        let details = fetch_details(client, hosts, account).await;
        enrich_with_details(&mut models, &details);
    }
    Ok(Some(models))
}

/// detail 接口返回的一条：`config_name → 原始对象`。
pub type DetailMap = std::collections::HashMap<String, Value>;

/// 拉取**原始模型条目**索引：`config_name`（含小写别名）→ 上游给的那一整段 JSON。
///
/// 为什么需要它：会话报文的 `custom_model` 必须**原样回传目录里的那段对象**。
/// 只给 `model_name` 时上游会静默换成默认模型，或者直接回
/// `{"code":995000,"message":"internal server error"}` —— HTTP 200 里套一个
/// 业务错误，看上去像上游挂了，其实是请求形状不完整。
///
/// 失败返回空 map（调用方当作"拿不到条目"，退回只发 model_name 的老行为）。
pub async fn fetch_model_objects(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> DetailMap {
    if account.access_token.as_deref().unwrap_or_default().is_empty() {
        return DetailMap::new();
    }
    match transport::get_json(
        client,
        &hosts.models_url(),
        &identity::base_headers(account, "application/json"),
    )
    .await
    {
        Ok(payload) => parse_model_objects(&payload),
        Err(_) => DetailMap::new(),
    }
}

/// 从 `/models` 响应里建立原始条目索引。
///
/// 与 [`parse_catalog`] 的差别：那个产出归一化的 [`UpstreamModel`]（面板用），
/// 这个保留**原样 JSON**（会话报文用）。两者都按 Agent → Work → Design 的
/// tier 顺序取第一次出现的条目，保证同名模型用的是能力最全的那份配置。
pub fn parse_model_objects(payload: &Value) -> DetailMap {
    let groups: &[Value] = payload
        .get("data")
        .and_then(|data| data.get("list"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut map = DetailMap::new();
    for tier in TIER_ORDER {
        for group in groups {
            let tier_name = group
                .get("function")
                .or_else(|| group.get("agent_type"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if tier_name != tier {
                continue;
            }
            for raw in group
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(id) = raw
                    .get("config_name")
                    .or_else(|| raw.get("configName"))
                    .or_else(|| raw.get("name"))
                    .or_else(|| raw.get("model_name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                else {
                    continue;
                };
                // 两种拼写各放一份：下游可能按小写名请求，而上游要精确拼写。
                map.entry(id.to_string()).or_insert_with(|| raw.clone());
                map.entry(id.to_ascii_lowercase())
                    .or_insert_with(|| raw.clone());
            }
        }
    }
    map
}

/// 拉 `get_detail_param` 的原生目录（元数据来源：输出上限 / 思考档位）。
///
/// 失败返回空 map 而不是 Err：这是**增强**步骤，拿不到就只是少了输出/档位，
/// 不该让整个渠道目录不可用。但失败原因必须打进日志 —— 否则面板上
/// 一排「输出 —」「档位 无」，用户和我们都无从判断是接口 404、鉴权被拒，
/// 还是响应结构又变了。
pub async fn fetch_details(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> DetailMap {
    let body = serde_json::json!({
        "function": "solo_work_lite",
        "config_names": null,
        "need_prompt": false,
        "current_config_info": null,
        "poly_prompt": true,
        "mode_type": null,
        "agent_type": null,
    });
    let url = hosts.detail_param_url();
    let payload = match transport::post_json(
        client,
        &url,
        &identity::detail_headers(account),
        &body,
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            // 带上上游原文：404 / 401 / 结构变更一眼可辨。
            eprintln!(
                "[trae] 元数据目录拉取失败 {url}：{}",
                error.describe()
            );
            return DetailMap::new();
        }
    };
    let map = parse_details(&payload);
    if map.is_empty() {
        // 请求成功但没有可用条目：把响应的**形状**打出来（截断），
        // 这通常是 config_info_list 换了位置或字段名。
        let preview = crate::gateway::channels::openai_compat::raw(&payload.to_string());
        eprintln!("[trae] 元数据目录为空（响应形状可能变了）：{preview}");
    } else {
        eprintln!("[trae] 元数据目录已加载 {} 个模型", map.len());
    }
    map
}

/// 解析 `get_detail_param`：模型可能在顶层，也可能在 `data` 里。
pub fn parse_details(payload: &Value) -> DetailMap {
    let list = payload
        .get("config_info_list")
        .and_then(Value::as_array)
        .or_else(|| {
            payload
                .get("data")
                .and_then(|data| data.get("config_info_list"))
                .and_then(Value::as_array)
        });
    let mut map = DetailMap::new();
    for raw in list.into_iter().flatten() {
        // 路由用的 profile（如 `custom_model_1M_text`）不是可选聊天模型。
        let usage = raw
            .get("usage")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !usage.is_empty() && usage != "chat_completion" {
            continue;
        }
        if raw.get("custom_models").and_then(Value::as_array).is_some_and(|list| !list.is_empty()) {
            continue;
        }
        let Some(name) = raw
            .get("config_name")
            .or_else(|| raw.get("configName"))
            .or_else(|| raw.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        map.insert(name.to_string(), raw.clone());
    }
    map
}

/// 用 detail 目录补齐 `models` 里缺失的元数据。
///
/// **只补空位，不覆盖**已有值：`/models` 给的 multimodal 等标记仍然优先。
/// 名字匹配大小写不敏感（两个接口的拼写偶有差异）。
pub fn enrich_with_details(models: &mut [UpstreamModel], details: &DetailMap) {
    if details.is_empty() {
        return;
    }
    // 建一张小写索引，避免每条模型都全表扫。
    let lowercase: std::collections::HashMap<String, Value> = details
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    for model in models.iter_mut() {
        let raw = details
            .get(&model.id)
            .or_else(|| lowercase.get(&model.id.to_ascii_lowercase()));
        let Some(raw) = raw else {
            continue;
        };
        let features = features_object(raw);
        let (context, windows) = parse_context_window(raw, features.as_ref());
        if model.context_length.is_none() {
            model.context_length = context;
        }
        if model.context_windows.is_empty() {
            model.context_windows = windows;
        }
        if model.max_output_tokens.is_none() {
            model.max_output_tokens = to_u64(raw.get("max_tokens"));
        }
        if model.reasoning_efforts.is_empty() {
            let (efforts, default, can_disable) = parse_reasoning(raw);
            model.reasoning_efforts = efforts;
            model.reasoning_default_effort = default;
            model.reasoning_can_disable = can_disable;
        }
        // `model_detail_list` 里的 dev profile 是真实上游 id，比 config 名更准。
        if model.context_length.is_none() {
            if let Some(list) = raw.get("model_detail_list").and_then(Value::as_array) {
                for detail in list {
                    let features = features_object(detail);
                    let (context, _) = parse_context_window(detail, features.as_ref());
                    if let Some(value) = context {
                        model.context_length = Some(value);
                        break;
                    }
                }
            }
        }
    }
}

/// 解析目录响应。
pub fn parse_catalog(payload: &Value) -> Vec<UpstreamModel> {
    let groups: &[Value] = payload
        .get("data")
        .and_then(|data| data.get("list"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut seen: HashSet<String> = HashSet::new();
    let mut models: Vec<UpstreamModel> = Vec::new();

    // 先按 tier 顺序走一遍，组内按上游顺序；重复名字只留第一次见到的。
    for tier in TIER_ORDER {
        for group in groups {
            let name = group
                .get("function")
                .or_else(|| group.get("agent_type"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if name != tier {
                continue;
            }
            for raw in group
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(model) = parse_model(raw) else { continue };
                if HIDDEN.contains(&model.id.as_str()) {
                    continue;
                }
                if !seen.insert(model.id.clone()) {
                    continue;
                }
                models.push(model);
            }
        }
    }
    models
}

fn parse_model(raw: &Value) -> Option<UpstreamModel> {
    let id = raw
        .get("config_name")
        .or_else(|| raw.get("configName"))
        .or_else(|| raw.get("name"))
        .or_else(|| raw.get("model_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();

    let number = |value: Option<&Value>| -> Option<u64> {
        match value? {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    };

    // `features` 可能是**对象**也可能是 **JSON 字符串**（上游两种都出现过）。
    // 只按对象读的话，字符串形态下所有 features 派生的字段全丢 ——
    // 上下文、最大轮次、工具能力都会变成空。
    let features = features_object(raw);

    // 上下文：真实来源有四处，按参考实现的优先级依次回落
    // （`context_window_size.max` → `features.context_windows.data.max_context`
    //   → `context_window_tokens.max`）。
    let (context_length, context_windows) = parse_context_window(raw, features.as_ref());
    // **输出上限的真实字段是 `max_tokens`**（不是 max_output_tokens）。
    let max_output_tokens = number(raw.get("max_tokens")).or_else(|| {
        // 少数模型把输出上限藏在 features 里。
        features
            .as_ref()
            .and_then(|map| map.get("max_output_tokens"))
            .and_then(|value| match value {
                Value::Number(number) => number.as_u64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            })
    });

    let multimodal = raw
        .get("multimodal")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let feature = |key: &str| {
        features
            .as_ref()
            .and_then(|map| map.get(key))
            .map(|value| match value {
                Value::Bool(flag) => *flag,
                Value::Number(number) => number.as_u64().unwrap_or(0) > 0,
                Value::String(text) => !text.is_empty() && text != "0",
                _ => false,
            })
            .unwrap_or(false)
    };

    // 目录是否**明确**声明了工具能力（两个键名任一为真）。
    //
    // 缺失 ≠ 不支持：把"有没有声明"单独记下来，交给矩阵决定要不要过滤。
    let declared_tools = feature("tool_call") || feature("supports_tool_call");
    // 图片能力同理：`features.multimodal.enable` 与顶层 `multimodal` 任一为真即算声明。
    let declared_vision = feature("multimodal") || multimodal;
    // 思考档位：真实字段是 `reasoning_effort_config.{options, default_level,
    // support_thinking}`（我之前按 `supported_efforts`/`default_effort` 读，
    // 那两个名字根本不存在，所以档位一直显示"无"）。
    let (efforts, default_effort, can_disable) = parse_reasoning(raw);

    Some(UpstreamModel {
        id,
        // 倍率取**当前实付价**：`features.consumption_rate.data.rate` 是标准价，
        // `features.activity_discount` / `features.discount` 里的
        // `consumption_rate` / `current.consumption_rate` 才是实际扣费倍率
        // （补贴、限时、会员优惠都会体现在这里）。
        credits: parse_credits(raw, features.as_ref()),
        context_length,
        max_output_tokens,
        vendor: Some("trae".into()),
        capabilities: ModelCapabilities {
            // `features.multimodal.enable` 比顶层 `multimodal` 更准确
            // （顶层字段偶有缺失，features 里才是权威）。
            vision: declared_vision,
            // Trae 的全部模型都走同一个原生工具协议（`tool_call` 只是目录里的
            // 一个可选标记，缺省不等于不支持）。目录没提就按"未声明"处理，
            // 让矩阵乐观放行 —— 早先这里回落成 false，配合"能力已声明"的判断就把
            // Trae 整个渠道判成"不支持工具调用"，带 tools 的请求全被本地拦掉。
            tool_calls: declared_tools,
            reasoning: feature("reasoning") || !efforts.is_empty(),
            declared_tools,
            declared_vision,
        },
        reasoning_efforts: efforts,
        reasoning_default_effort: default_effort,
        reasoning_can_disable: can_disable,
        always_reasoning: false,
        temperature: None,
        top_p: None,
        context_windows,
        ..UpstreamModel::default()
    })
}

/// 取 `features` 对象：它可能是对象，也可能是 **JSON 字符串**。
///
/// 上游两种形态都出现过。只按对象读时，字符串形态下所有由 features
/// 派生的能力位与上下文全部丢失 —— 这正是"上下文一直是空"的一个来源。
fn features_object(raw: &Value) -> Option<serde_json::Map<String, Value>> {
    match raw.get("features") {
        Some(Value::Object(map)) => Some(map.clone()),
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| value.as_object().cloned()),
        _ => None,
    }
}

/// 把任意标量/单元素数组解析成 u64。
fn to_u64(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        // `max: [1000000]`
        Value::Array(items) => items.iter().find_map(|item| to_u64(Some(item))),
        _ => None,
    }
}

/// 解析上下文上限与档位，按参考实现的优先级依次回落。
///
/// 四个来源（任一命中即可，全部缺失才认为"上游没给"）：
/// ① `context_window_size.max`（最常见）
/// ② `features.context_windows.data.max_context`
/// ③ `context_window_tokens.max`
/// ④ 扁平字段 `max_input_tokens` / `context_length`（老版本）
///
/// 默认档同理：`context_window_size.default` → `features...dev_context`
/// → `context_window_tokens.dev`。
fn parse_context_window(
    raw: &Value,
    features: Option<&serde_json::Map<String, Value>>,
) -> (Option<u64>, Vec<u64>) {
    let window = raw
        .get("context_window_size")
        .or_else(|| raw.get("contextWindowSize"))
        .or_else(|| raw.get("context_window"))
        .or_else(|| raw.get("contextWindow"));

    // features 里的上下文数据（②）。
    let feature_cw = features
        .and_then(|map| map.get("context_windows"))
        .and_then(|value| match value {
            Value::Object(map) => Some(map.clone()),
            Value::String(text) => serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|parsed| parsed.as_object().cloned()),
            _ => None,
        });
    let feature_data = feature_cw.as_ref().and_then(|cw| cw.get("data"));

    // 分开的 tokens 形状（③）：**这是实测响应里的真实字段**。
    //   "context_window_tokens": { "dev": 200000, "max": 1000000 }
    // 注意 `max` 可能是 **0**（Work 档就是这样，表示"不额外放开"），
    // 0 必须当成"没有更大档位"而回落 dev，不能当成上限 0。
    let tokens = raw
        .get("context_window_tokens")
        .or_else(|| raw.get("contextWindowTokens"));
    let positive = |value: Option<&Value>| -> Option<u64> { to_u64(value).filter(|v| *v > 0) };

    let max = positive(window.and_then(|w| w.get("max")))
        .or_else(|| positive(feature_data.and_then(|data| data.get("max_context"))))
        .or_else(|| positive(tokens.and_then(|tokens| tokens.get("max"))))
        .or_else(|| positive(raw.get("max_input_tokens")))
        .or_else(|| positive(raw.get("maxInputTokens")))
        .or_else(|| positive(raw.get("context_length")));

    let default = positive(window.and_then(|w| w.get("default")))
        .or_else(|| positive(feature_data.and_then(|data| data.get("dev_context"))))
        .or_else(|| positive(tokens.and_then(|tokens| tokens.get("dev"))));

    // 上限优先用 max；没有 max 时用 default（有些模型只给一个档位）。
    let context = max.or(default);
    let mut windows: Vec<u64> = Vec::new();
    // `features.context_windows.data.max_context_list` 会列出全部可选档位。
    if let Some(listed) = feature_data
        .and_then(|data| data.get("max_context_list"))
        .and_then(Value::as_array)
    {
        for item in listed {
            if let Some(value) = to_u64(Some(item)) {
                if !windows.contains(&value) {
                    windows.push(value);
                }
            }
        }
    }
    for candidate in [default, max] {
        if let Some(value) = candidate {
            if !windows.contains(&value) {
                windows.push(value);
            }
        }
    }
    // 面板按"从小到大"展示档位更直观。
    windows.sort_unstable();
    (context, windows)
}

/// 倍率：取**当前实付价**（`x0.20` 这种形式）。
///
/// 真实位置在 `features` 这个 JSON 字符串里，有两层语义：
/// · `consumption_rate.data.rate` —— 标准价（挂牌价）；
/// · `activity_discount` / `discount` 里的 `consumption_rate` —— **实际扣费**。
///
/// 优先级（取第一个能解析的）：
/// ① `discount.data.consumption_rate`（会员/定向折扣，已生效才用）
/// ② `activity_discount.data.current.consumption_rate`（补贴/限时/错峰当前价）
/// ③ `consumption_rate.data.rate`（标准价兜底）
///
/// 面板只显示这一个数字（用户要求"就按当前账号实付价显示"），
/// 标准价与优惠幅度不额外展示 —— 想扩成 `x0.4 → x0.2` 时，
/// 在这里多返回一个字段即可，调用方不必改。
fn parse_credits(
    raw: &Value,
    features: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    // 少量老版本可能直接把倍率放在条目顶层，先认它。
    for key in ["credits", "credit_rate", "creditRate", "multiplier"] {
        if let Some(value) = raw.get(key) {
            if let Some(text) = rate_text(value) {
                return Some(text);
            }
        }
    }

    let features = features?;
    let section = |name: &str| -> Option<&serde_json::Map<String, Value>> {
        features.get(name).and_then(Value::as_object)
    };
    let number = |map: Option<&serde_json::Map<String, Value>>, path: &[&str]| -> Option<f64> {
        let mut current = Value::Null;
        let mut cursor: Option<&Value> = None;
        for (index, key) in path.iter().enumerate() {
            let source = if index == 0 {
                map?.get(*key)?
            } else {
                cursor?.get(*key)?
            };
            cursor = Some(source);
            current = source.clone();
        }
        current.as_f64()
    };

    // ① 折扣（`discount.data.consumption_rate`）。
    if let Some(discount) = section("discount") {
        if discount.get("enable").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(rate) = number(Some(discount), &["data", "consumption_rate"]) {
                return Some(format_rate(rate));
            }
        }
    }
    // ② 活动/补贴当前价（`activity_discount.data.current.consumption_rate`）。
    if let Some(activity) = section("activity_discount") {
        if activity.get("enable").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(rate) = number(Some(activity), &["data", "current", "consumption_rate"]) {
                return Some(format_rate(rate));
            }
        }
    }
    // ③ 标准价兜底。
    if let Some(consumption) = section("consumption_rate") {
        if consumption.get("enable").and_then(Value::as_bool).unwrap_or(true) {
            if let Some(rate) = number(Some(consumption), &["data", "rate"]) {
                return Some(format_rate(rate));
            }
        }
    }
    None
}

/// 顶层 `credits` 这类字符串/数字 → 展示文本。
fn rate_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            // 已经是 `x0.5` 形式就原样，`0.5` 则补上 x。
            let trimmed = text.trim();
            Some(if trimmed.starts_with('x') || trimmed.starts_with('X') {
                trimmed.to_string()
            } else {
                format!("x{trimmed}")
            })
        }
        Value::Number(number) => Some(format_rate(number.as_f64().unwrap_or(0.0))),
        _ => None,
    }
}

/// 倍率格式化：统一两位小数、去掉多余的 0，并加上 `x` 前缀。
///
/// `0.2` → `x0.20`，`0.78` → `x0.78`，`1.5` → `x1.50`，
/// `0` → `x0.00`（面板会据此判定"免费"，见 `credits_is_free`）。
fn format_rate(rate: f64) -> String {
    if !rate.is_finite() || rate < 0.0 {
        return "x0.00".to_string();
    }
    // 两位小数足够表达上游精度（实测都是 0.06 / 0.12 / 0.78 这类）。
    format!("x{rate:.2}")
}

fn parse_reasoning(raw: &Value) -> (Vec<String>, Option<String>, Option<bool>) {
    let config = raw
        .get("reasoning_effort_config")
        .or_else(|| raw.get("reasoningEffortConfig"));
    let Some(config) = config else {
        return (Vec::new(), None, None);
    };
    let text_list = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(text) => Some(text.clone()),
                        Value::Object(map) => map
                            .get("effort")
                            .or_else(|| map.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let container = config
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(config);
    // 真实字段名是 `options` / `default_level`（实测响应）：
    //   {"support_thinking": true, "options": ["light","high","extra_high"],
    //    "default_level": "extra_high"}
    // 旧名字 `supported_efforts` / `default_effort` 保留作兼容兜底。
    let efforts = text_list(
        container
            .get("options")
            .or_else(|| container.get("supported_efforts"))
            .or_else(|| container.get("supportedEfforts"))
            .or_else(|| container.get("efforts")),
    );
    let default = container
        .get("default_level")
        .or_else(|| container.get("default_effort"))
        .or_else(|| container.get("defaultEffort"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let can_disable = container
        .get("can_disable_thinking")
        .or_else(|| container.get("canDisableThinking"))
        .and_then(Value::as_bool);
    // `support_thinking: false` 的模型即使带了 options 也不该显示档位
    // （实测 Doubao 系列就是 false + options:null，Minimax 才是 true）。
    if container
        .get("support_thinking")
        .and_then(Value::as_bool)
        .is_some_and(|supported| !supported)
    {
        return (Vec::new(), None, can_disable);
    }
    (efforts, default, can_disable)
}

/// 目录与快照合并：实时条目优先，快照只补**名字**。
///
/// 快照刻意不带上下文/输出（见 `SNAPSHOT` 的说明），所以这里不存在
/// "用快照补齐元数据"这一步 —— 补进来的元数据不可信，宁可留空。
pub fn merge(live: Vec<UpstreamModel>) -> Vec<UpstreamModel> {
    let mut merged = live;
    let existing: HashSet<String> = merged.iter().map(|model| model.id.clone()).collect();
    for entry in snapshot() {
        if !existing.contains(&entry.id) {
            merged.push(entry);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_tier_wins_and_duplicates_are_dropped() {
        let payload = json!({"data": {"list": [
            {"function": "solo_work_remote", "models": [
                {"config_name": "glm-5.3", "max_input_tokens": 200000},
                {"config_name": "work-only"}
            ]},
            {"function": "solo_agent_remote", "models": [
                {"config_name": "glm-5.3", "max_input_tokens": 1000000, "multimodal": true},
                {"config_name": "auto"}
            ]}
        ]}});
        let models = parse_catalog(&payload);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        // Agent 档优先：glm-5.3 用的是 Agent 那份（1M 上下文），不是 Work 那份。
        assert_eq!(ids, vec!["glm-5.3", "work-only"]);
        assert_eq!(models[0].context_length, Some(1_000_000));
        assert!(models[0].capabilities.vision);
        // 内部虚拟模型不上架。
        assert!(!ids.contains(&"auto"));
    }

    #[test]
    fn reasoning_config_is_parsed_from_both_shapes() {
        let payload = json!({"data": {"list": [
            {"function": "solo_agent_remote", "models": [
                {"config_name": "a", "reasoning_effort_config": {
                    "supported_efforts": ["low", "high"], "default_effort": "high", "can_disable_thinking": true
                }},
                {"config_name": "b", "reasoning_effort_config": [{"efforts": ["max"]}]}
            ]}
        ]}});
        let models = parse_catalog(&payload);
        assert_eq!(models[0].reasoning_efforts, vec!["low", "high"]);
        assert_eq!(models[0].reasoning_default_effort.as_deref(), Some("high"));
        assert_eq!(models[0].reasoning_can_disable, Some(true));
        assert!(models[0].capabilities.reasoning, "有档位说明支持思考");
        assert_eq!(models[1].reasoning_efforts, vec!["max"]);
    }

    /// 真实模型元数据形状：上下文在 `context_window_size`，输出上限是 `max_tokens`。
    /// 这两个字段读错就是面板上满屏的 "?"。
    #[test]
    fn real_metadata_shape_yields_context_and_output_limits() {
        let payload = json!({"data": {"list": [
            {"function": "solo_agent_remote", "models": [
                {
                    "name": "glm-5.3",
                    "max_tokens": 64000,
                    // max 是数组（真实响应里就长这样）。
                    "context_window_size": {"default": 200000, "max": [1000000]},
                    "multimodal": true
                },
                {
                    "name": "kimi-k2.6",
                    "max_tokens": 32000,
                    // max 是数字。
                    "context_window_size": {"default": 256000, "max": 1000000}
                },
                {
                    // 只有 default 没有 max：上下文用 default。
                    "name": "minimax-m3",
                    "max_tokens": "128000",
                    "context_window_size": {"default": 512000}
                }
            ]}
        ]}});
        let models = parse_catalog(&payload);
        assert_eq!(models[0].context_length, Some(1_000_000), "max 数组取首项");
        assert_eq!(models[0].max_output_tokens, Some(64_000), "输出上限读 max_tokens");
        assert_eq!(models[0].context_windows, vec![200_000, 1_000_000], "档位要列出来");
        assert!(models[0].capabilities.vision, "multimodal 才是视觉能力位");

        assert_eq!(models[1].context_length, Some(1_000_000), "max 数字也要认");
        assert_eq!(models[1].max_output_tokens, Some(32_000));

        assert_eq!(models[2].context_length, Some(512_000), "没有 max 时用 default");
        assert_eq!(models[2].max_output_tokens, Some(128_000), "字符串数字也要认");
    }

    /// 倍率：从 `features` 的 JSON 字符串里取**当前实付价**。
    ///
    /// 用真机抓包的三个代表模型做夹具，覆盖三种计费形态：
    /// · glm-5.3 —— 标准价 0.78 + 会员折扣后 0.39；
    /// · Doubao-Seed-2.1-Turbo —— 补贴后 0.2（标准价也是 0.2）；
    /// · minimax-m3.1-flash —— 只有标准价 0.09。
    #[test]
    fn credits_use_the_effective_rate_from_features() {
        let payload = json!({"data": {"list": [{"function": "solo_agent_remote", "models": [
            {
                "name": "glm-5.3",
                "features": "{\"consumption_rate\":{\"enable\":true,\"data\":{\"rate\":0.78}},\"discount\":{\"enable\":true,\"subKey\":\"member_discount\",\"data\":{\"original_consumption_rate\":0.78,\"consumption_rate\":0.39,\"member_discount\":50}}}"
            },
            {
                "name": "Doubao-Seed-2.1-Turbo",
                "features": "{\"consumption_rate\":{\"enable\":true,\"data\":{\"rate\":0.2}},\"activity_discount\":{\"enable\":true,\"subKey\":\"subsidy_member_discount\",\"data\":{\"current\":{\"discount_type\":\"subsidy\",\"before_consumption_rate\":0.4,\"consumption_rate\":0.2}}}}"
            },
            {
                "name": "minimax-m3.1-flash",
                "features": "{\"consumption_rate\":{\"enable\":true,\"data\":{\"rate\":0.09}}}"
            }
        ]}]}});
        let models = parse_catalog(&payload);
        let rate = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .and_then(|model| model.credits.clone())
        };
        // 折扣生效时取折扣价（用户要的是"当前账号实付"）。
        assert_eq!(rate("glm-5.3").as_deref(), Some("x0.39"));
        assert_eq!(rate("Doubao-Seed-2.1-Turbo").as_deref(), Some("x0.20"));
        assert_eq!(rate("minimax-m3.1-flash").as_deref(), Some("x0.09"));
    }

    /// `features` 是**字符串**时才藏得住这些值；解析必须走 JSON 二次解码。
    #[test]
    fn credits_read_features_as_object_too() {
        let payload = json!({"data": {"list": [{"function": "solo_agent_remote", "models": [
            {"name": "as-object",
             "features": {"consumption_rate": {"enable": true, "data": {"rate": 0.5}}}}
        ]}]}});
        let models = parse_catalog(&payload);
        assert_eq!(models[0].credits.as_deref(), Some("x0.50"));
    }

    /// 上游完全不给计费信息时必须是 None（面板显示「—」），不能编一个 0 出来。
    #[test]
    fn credits_are_none_when_upstream_says_nothing() {
        let payload = json!({"data": {"list": [
            {"function": "solo_agent_remote", "models": [{"name": "plain-model"}]}
        ]}});
        let models = parse_catalog(&payload);
        assert_eq!(
            models[0].credits, None,
            "没有 features 时不能伪造倍率"
        );
    }

    /// 上下文：真实字段是 `context_window_tokens.{dev,max}`。
    /// `max` 为 0 表示"不放开更大档位"（Work 档就是这样），必须回落 dev。
    #[test]
    fn context_uses_dev_and_max_with_zero_meaning_no_larger_window() {
        let payload = json!({"data": {"list": [{"function": "solo_agent_remote", "models": [
            {"name": "with-max", "context_window_tokens": {"dev": 200000, "max": 1000000}},
            {"name": "max-is-zero", "context_window_tokens": {"dev": 256000, "max": 0}}
        ]}]}});
        let models = parse_catalog(&payload);
        let get = |id: &str| models.iter().find(|model| model.id == id).unwrap();
        assert_eq!(get("with-max").context_length, Some(1_000_000));
        assert_eq!(
            get("max-is-zero").context_length,
            Some(256_000),
            "max=0 要回落 dev，不能当成上限 0"
        );
    }

    /// 思考档位：真实字段是 `options` / `default_level` / `support_thinking`。
    #[test]
    fn reasoning_uses_options_and_default_level() {
        let payload = json!({"data": {"list": [{"function": "solo_agent_remote", "models": [
            {"name": "with-effort", "reasoning_effort_config": {
                "support_thinking": true,
                "options": ["light", "high", "extra_high"],
                "default_level": "extra_high"
            }},
            {"name": "no-thinking", "reasoning_effort_config": {
                "support_thinking": false, "options": null, "default_level": ""
            }}
        ]}]}});
        let models = parse_catalog(&payload);
        let get = |id: &str| models.iter().find(|model| model.id == id).unwrap();
        assert_eq!(get("with-effort").reasoning_efforts, vec!["light", "high", "extra_high"]);
        assert_eq!(get("with-effort").reasoning_default_effort.as_deref(), Some("extra_high"));
        assert!(
            get("no-thinking").reasoning_efforts.is_empty(),
            "support_thinking=false 时不该显示档位"
        );
    }

    /// **端到端**：`/models` 只给 slim object（有 multimodal、没上下文），
    /// 元数据从 `get_detail_param` 补进来 —— 这是面板显示上下文的真实链路。
    #[test]
    fn details_enrich_slim_catalog_entries() {
        // 第一个接口：只有名字与 multimodal。
        let slim = json!({"data": {"list": [
            {"function": "solo_agent_remote", "models": [
                {"name": "DeepSeek-V4-Flash-Official", "multimodal": false},
                {"name": "glm-5.3"}
            ]}
        ]}});
        let mut models = parse_catalog(&slim);
        assert_eq!(models[0].context_length, None, "slim 目录没有上下文");

        // 第二个接口：真正的元数据。
        let details = parse_details(&json!({"config_info_list": [
            {
                "config_name": "DeepSeek-V4-Flash-Official",
                "usage": "chat_completion",
                "max_tokens": 64000,
                "context_window_size": {"default": 200000, "max": [1000000]},
                "reasoning_effort_config": {
                    "supported_efforts": ["low", "high"], "default_effort": "high"
                }
            },
            {
                "config_name": "glm-5.3",
                "usage": "chat_completion",
                "max_tokens": 48000,
                "context_window_size": {"default": 200000, "max": 1000000}
            }
        ]}));
        assert_eq!(details.len(), 2);

        enrich_with_details(&mut models, &details);
        let flash = models
            .iter()
            .find(|m| m.id == "DeepSeek-V4-Flash-Official")
            .expect("模型还在");
        assert_eq!(
            flash.context_length,
            Some(1_000_000),
            "上下文必须从 detail 接口补进来"
        );
        assert_eq!(flash.max_output_tokens, Some(64_000), "输出上限同理");
        assert_eq!(flash.reasoning_efforts, vec!["low", "high"], "思考档位同理");

        let glm = models.iter().find(|m| m.id == "glm-5.3").expect("模型还在");
        assert_eq!(glm.context_length, Some(1_000_000));
    }

    /// detail 目录里的路由 profile（usage 不是 chat_completion）不该混进来。
    #[test]
    fn detail_parser_skips_routing_profiles() {
        let details = parse_details(&json!({"config_info_list": [
            {"config_name": "custom_model_1M_text", "usage": "routing"},
            {"config_name": "real-model", "usage": "chat_completion"},
            {"config_name": "no-usage"}
        ]}));
        assert!(details.contains_key("real-model"));
        assert!(details.contains_key("no-usage"), "缺 usage 按聊天模型处理");
        assert!(!details.contains_key("custom_model_1M_text"));
    }

    /// 名字大小写不一致也要能对上（两个接口偶有差异）。
    #[test]
    fn detail_matching_is_case_insensitive() {
        let mut models = vec![UpstreamModel {
            id: "Brand-New-Model".into(),
            ..UpstreamModel::default()
        }];
        let details = parse_details(&json!({"config_info_list": [
            {"config_name": "brand-new-model", "context_window_size": {"max": 500000}}
        ]}));
        enrich_with_details(&mut models, &details);
        assert_eq!(models[0].context_length, Some(500_000));
    }

    /// 已有多元数据不能被 detail 覆盖（`/models` 的 multimodal 优先）。
    #[test]
    fn details_never_overwrite_existing_metadata() {
        let mut models = vec![UpstreamModel {
            id: "m".into(),
            context_length: Some(123),
            max_output_tokens: Some(456),
            ..UpstreamModel::default()
        }];
        let details = parse_details(&json!({"config_info_list": [
            {"config_name": "m", "max_tokens": 999, "context_window_size": {"max": 999}}
        ]}));
        enrich_with_details(&mut models, &details);
        assert_eq!(models[0].context_length, Some(123), "不覆盖已有值");
        assert_eq!(models[0].max_output_tokens, Some(456));
    }

    #[test]
    fn empty_catalog_is_treated_as_failure_not_an_empty_listing() {
        // 上游凭据不可用时会返回 200 + 空列表；解析成空 vec，
        // 由 fetch 换成 Ok(None) 表示"这次没拉到"。
        assert!(parse_catalog(&json!({"data": {"list": []}})).is_empty());
        assert!(parse_catalog(&json!({})).is_empty());
        // 快照**故意为空**：曾经硬编码的 6 个名字与实时目录拼写不一致，
        // 会在面板上产生"影子模型"（重复显示、元数据全空）。
        assert!(
            snapshot().is_empty(),
            "快照不能编造模型名，否则会出现重复且无元数据的条目"
        );
    }

    #[test]
    fn merge_keeps_live_metadata_and_adds_nothing_else() {
        let live = vec![UpstreamModel {
            id: "glm-5.3".into(),
            context_length: Some(1_000_000),
            ..UpstreamModel::default()
        }];
        let merged = merge(live);
        assert_eq!(merged.len(), 1, "空快照不该往清单里加任何条目");
        assert_eq!(
            merged[0].context_length,
            Some(1_000_000),
            "实时元数据必须原样保留"
        );
    }
}
