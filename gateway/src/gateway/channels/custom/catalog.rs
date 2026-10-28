//! 模型清单：配置解析 + 通用「获取模型」。
//!
//! 两个方向：**读配置**（用户在面板里填的/勾选的模型）与**拉上游**（按地址拉一次清单）。
//! 解析一律宽松 —— 各家方言的字段名互不相同，只认一种就必然读不到别的。

use serde_json::Value;

use crate::gateway::{
    channels::{AccountSnapshot, ModelCapabilities, UpstreamModel},
    errors::GatewayError,
    transport,
};

use super::{join_url, UpstreamProtocol};

/// 配置里的模型清单。既接受纯字符串（只要 id），也接受带元数据的对象。
pub(super) fn configured_models(options: &Value) -> Vec<UpstreamModel> {
    let Some(items) = options.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for item in items {
        let parsed = match item {
            Value::String(id) => {
                let id = normalise_model_id(id);
                (!id.is_empty()).then(|| UpstreamModel {
                    id,
                    ..UpstreamModel::default()
                })
            }
            Value::Object(_) => parse_model_entry(item),
            _ => None,
        };
        if let Some(model) = parsed {
            if !models.iter().any(|existing: &UpstreamModel| existing.id == model.id) {
                models.push(model);
            }
        }
    }
    models
}

/// 解析单条模型。**宽松**：不同方言的字段名互不认识，缺字段就留空。
///
/// 字段别名来自各家实际返回：OpenAI 用 `context_length`/`max_output_tokens`，
/// Anthropic 用 `max_input_tokens`/`max_tokens`，Google 用 `inputTokenLimit`/`outputTokenLimit`，
/// LM Studio 用 `max_context_length`。只认一种就必然读不到别的。
fn parse_model_entry(value: &Value) -> Option<UpstreamModel> {
    let id = ["id", "slug", "name", "key", "model", "model_id"]
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(normalise_model_id)
        .filter(|id| !id.is_empty())?;

    let number = |keys: &[&str]| -> Option<u64> {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(Value::as_u64))
            .filter(|value| *value > 0)
    };
    let flag = |keys: &[&str]| -> bool {
        keys.iter()
            .any(|key| value.get(*key).and_then(Value::as_bool).unwrap_or(false))
    };
    // 「键是否存在」——用于区分"明确填了 false"与"根本没填"。
    let present = |keys: &[&str]| -> bool {
        keys.iter().any(|key| value.get(*key).is_some())
    };
    let nested_present = |outer: &str, keys: &[&str]| -> bool {
        value
            .get(outer)
            .map(|node| keys.iter().any(|key| node.get(*key).is_some()))
            .unwrap_or(false)
    };

    let nested_flag = |outer: &str, keys: &[&str]| -> bool {
        value
            .get(outer)
            .map(|node| flag_on(node, keys))
            .unwrap_or(false)
    };

    let credits = value
        .get("credits")
        .or_else(|| value.get("credit"))
        .and_then(|node| {
            node.as_str()
                .map(str::to_string)
                .or_else(|| node.as_f64().map(|number| format!("x{number:.2}")))
        });

    let context_length = number(&[
        "context_length",
        "context_window",
        "max_input_tokens",
        "maxInputTokens",
        "max_context_length",
        "inputTokenLimit",
    ]);
    // 注意：`max_tokens` 在 Anthropic 里是**输出**上限，在别处也可能是输入 ——
    // 这里只在没有更明确字段时用它当输出，与各家目录的实际用法一致。
    let max_output_tokens = number(&["max_output_tokens", "maxOutputTokens", "outputTokenLimit"])
        .or_else(|| {
            number(&["max_tokens"]).filter(|value| {
                context_length.is_none_or(|context| *value < context)
            })
        });

    Some(UpstreamModel {
        id,
        credits,
        context_length,
        max_output_tokens,
        vendor: value
            .get("vendor")
            .or_else(|| value.get("owned_by"))
            .and_then(Value::as_str)
            .map(str::to_string),
        capabilities: ModelCapabilities {
            vision: flag(&["supportsImages", "supports_images", "vision"])
                || nested_flag("capabilities", &["vision", "image", "images"]),
            tool_calls: flag(&["supportsToolCall", "supports_tool_call", "tool_calls"])
                || nested_flag("capabilities", &["tool_calls", "tools", "function_calling"]),
            reasoning: flag(&["supportsReasoning", "supports_reasoning", "reasoning"])
                || nested_flag("capabilities", &["reasoning", "thinking"]),
            // 自定义供应商的能力位由**用户自己填**：键存在即算声明（哪怕值是 false）。
            // 键完全不存在 = 用户没填 = 不声明，交给矩阵乐观放行。
            // 注意不能用 `flag(..)` 判断"是否声明"：那会把明确填的 false 也当成没填。
            declared_tools: present(&["supportsToolCall", "supports_tool_call", "tool_calls"])
                || nested_present("capabilities", &["tool_calls", "tools", "function_calling"]),
            declared_vision: present(&["supportsImages", "supports_images", "vision"])
                || nested_present("capabilities", &["vision", "image", "images"]),
        },
        reasoning_efforts: value
            .get("reasoning_efforts")
            .or_else(|| value.get("reasoning_efforts_supported"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        reasoning_default_effort: value
            .get("reasoning_default_effort")
            .and_then(Value::as_str)
            .map(str::to_string),
        always_reasoning: flag(&["always_reasoning", "onlyReasoning"]),
        // 配置与实时拉取都是**当前**事实；同时也是用户显式声明的模型。
        from_snapshot: false,
        user_declared: true,
        ..UpstreamModel::default()
    })
}

fn flag_on(node: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| node.get(*key).and_then(Value::as_bool).unwrap_or(false))
}

/// 模型 id 归一：Google 返回 `models/gemini-2.5-pro`，前缀对客户端无意义。
fn normalise_model_id(id: &str) -> String {
    id.trim()
        .strip_prefix("models/")
        .unwrap_or(id.trim())
        .to_string()
}

/// 「获取模型」的候选地址（按顺序试，先命中者胜）。
///
/// 规则来自通用实践（cc-switch / Cherry Studio 同一套）：
/// 地址已经是版本段（`/v1`、`/api/coding/paas/v4`）时只补 `/models` ——
/// 再补一次 `/v1` 会变成 `.../v4/v1/models`，必然 404；
/// 否则补 `/v1/models`。带 Anthropic 子路径的（`/api/anthropic`）再回退到根。
pub fn models_url_candidates(base_url: &str, override_url: Option<&str>) -> Vec<String> {
    if let Some(url) = override_url.map(str::trim).filter(|url| !url.is_empty()) {
        return vec![url.to_string()];
    }
    let mut base = base_url.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return Vec::new();
    }
    // 完整端点（用户直接粘了 `/chat/completions`）：截到版本根。
    for suffix in ["/chat/completions", "/messages", "/completions"] {
        if let Some(index) = base.find(suffix) {
            base.truncate(index);
            break;
        }
    }
    let base = base.trim_end_matches('/').to_string();

    let mut candidates: Vec<String> = Vec::new();
    let mut push = |url: String| {
        if !candidates.iter().any(|existing| existing == &url) {
            candidates.push(url);
        }
    };
    if has_version_suffix(&base) {
        push(join_url(&base, "models"));
        // 版本段不是 /v1 时，`{base}/v1/models` 也可能是对的（少数服务两个都挂）。
        if !base.ends_with("/v1") {
            push(join_url(&base, "v1/models"));
        }
    } else {
        push(join_url(&base, "v1/models"));
        push(join_url(&base, "models"));
    }
    // Anthropic 兼容子路径：剥掉后再试根地址。
    if let Some(root) = strip_anthropic_suffix(&base) {
        push(join_url(&root, "v1/models"));
        push(join_url(&root, "models"));
    }
    candidates.truncate(4);
    candidates
}

/// 最后一段形如 `v1` / `v1beta` / `v4`？
fn has_version_suffix(base: &str) -> bool {
    let last = base.rsplit('/').next().unwrap_or_default();
    let Some(rest) = last.strip_prefix(['v', 'V']) else {
        return false;
    };
    rest.chars().next().is_some_and(|first| first.is_ascii_digit())
}

/// 剥掉 Anthropic 兼容子路径（`/api/anthropic`、`/claudecode` …），拿回根地址。
fn strip_anthropic_suffix(base: &str) -> Option<String> {
    const SUFFIXES: [&str; 6] = [
        "/api/claudecode",
        "/api/anthropic",
        "/anthropic",
        "/claudecode",
        "/claude",
        "/coding",
    ];
    SUFFIXES
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .map(|root| root.trim_end_matches('/').to_string())
        .filter(|root| !root.is_empty() && root.contains("//"))
}

/// 通用「获取模型」：按候选地址逐个试，404/405 视为「这个地址上没挂」而下一位，
/// 其余错误（401/403/超时/非 JSON）如实上报 —— 那是用户要修的东西。
///
/// 面板填供应商时用它拉一次清单；也用于「重新获取」。
pub async fn fetch_models(
    client: &reqwest::Client,
    base_url: &str,
    models_url: Option<&str>,
    api_key: &str,
    protocol: &str,
) -> Result<Vec<UpstreamModel>, GatewayError> {
    let candidates = models_url_candidates(base_url, models_url);
    if candidates.is_empty() {
        return Err(GatewayError::invalid_request("请先填写供应商地址"));
    }
    let protocol = UpstreamProtocol::parse(protocol);
    let account = AccountSnapshot {
        access_token: (!api_key.trim().is_empty()).then(|| api_key.trim().to_string()),
        ..AccountSnapshot::default()
    };
    let headers = auth_headers_for(protocol, &account);

    let mut last_error: Option<GatewayError> = None;
    for url in candidates {
        match transport::get_json(client, &url, &headers).await {
            Ok(payload) => {
                let models = parse_model_list(&payload);
                if models.is_empty() {
                    // 端点通了但没解析出模型：多半不是模型清单接口，换下一个候选。
                    last_error = Some(GatewayError::invalid_request(format!(
                        "{url} 返回的内容里没有模型清单"
                    )));
                    continue;
                }
                return Ok(models);
            }
            Err(error) => {
                // status == 0 表示根本没拿到 HTTP 状态码（连接/超时）：如实上报。
                // 404/405 = 该地址没挂这个端点，换下一个候选。
                if matches!(error.status, 404 | 405) {
                    last_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        GatewayError::unsupported("所有候选地址都没有模型清单端点，请确认地址或手动填写模型")
    }))
}

/// 模型清单请求的鉴权头。与对话报文同一套规则（按协议选 Bearer / x-api-key）。
pub(super) fn auth_headers_for(
    protocol: UpstreamProtocol,
    account: &AccountSnapshot,
) -> Vec<(String, String)> {
    let mut headers = vec![("Accept".into(), "application/json".into())];
    headers.extend(super::protocol_auth_headers(protocol, &account));
    headers
}

/// 解析模型清单响应。**宽松**：`data[]` / `models[]` / 顶层数组都认，
/// 每条只取能给到的字段，一条坏数据不影响其余条目。
///
/// 各家的形状：OpenAI `{"data":[{"id"}]}`、Anthropic `{"data":[{"id","display_name"}]}`、
/// Google `{"models":[{"name":"models/gemini-…"}]}`、Ollama `{"models":[{"name"}]}`、
/// LM Studio `{"models":[{"key"}]}`、智谱 `{"models":[{"slug"}]}`、Together 直接是数组。
pub fn parse_model_list(payload: &Value) -> Vec<UpstreamModel> {
    let arrays: Vec<&Vec<Value>> = [
        payload.get("data"),
        payload.get("models"),
        payload.get("model_list"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_array)
    .collect();

    let mut entries: Vec<&Value> = arrays.iter().flat_map(|list| list.iter()).collect();
    if entries.is_empty() {
        if let Some(list) = payload.as_array() {
            entries = list.iter().collect();
        }
    }

    let mut models: Vec<UpstreamModel> = Vec::new();
    for entry in entries {
        let parsed = match entry {
            Value::String(id) => {
                let id = normalise_model_id(id);
                (!id.is_empty()).then(|| UpstreamModel {
                    id,
                    ..UpstreamModel::default()
                })
            }
            Value::Object(_) => parse_model_entry(entry),
            _ => None,
        };
        if let Some(model) = parsed {
            if !models.iter().any(|existing| existing.id == model.id) {
                models.push(model);
            }
        }
    }
    models
}
