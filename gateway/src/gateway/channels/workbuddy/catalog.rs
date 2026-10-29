//! 模型目录：`GET /v3/config` 的解析、裁剪与「实时目录 ∪ 内置快照」合并。
//!
//! 三条硬规则，都是实测踩出来的：
//! ① `/v3/config` 只有桌面 UA 才返回完整清单，未知 UA 直接 400（code 12403）；
//! ② 凭据不可用时上游返回 **HTTP 200 + 空列表** —— 必须当失败回落，
//!    绝不能对外发布「这个渠道没有模型」；
//! ③ 实时目录里的补全通道与变体后缀要剔掉，否则客户端 pin 到什么就用不了什么。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::gateway::{
    channels::{AccountSnapshot, ModelCapabilities, UpstreamModel},
    transport,
};

use super::identity::{self, Hosts};

pub const CHAT_PATH: &str = "/v2/chat/completions";
pub const CONFIG_PATH: &str = "/v3/config";

/// 清洗规则是共通的（矩阵构建也用同一份），实现在 `channels` 层。
pub use crate::gateway::channels::{credits_is_free, is_chat_model};

/// 变体后缀：实测「裸名免费、变体收费」，只广告裸名。
const VARIANT_SUFFIXES: [&str; 2] = ["-sg", "-x"];

/// 国内版固定广告顺序：表内模型排前面，实时独有的按上游顺序追加。
pub const CN_UI_ORDER: [&str; 14] = [
    "hy4-preview-f",
    "hy3",
    "deepseek-v4.1-flash",
    "glm-5.3",
    "glm-5.3-flash",
    "glm-5.2",
    "glm-5.1",
    "glm-5v-turbo",
    "minimax-m3",
    "kimi-k3-1",
    "kimi-k2.8-preview",
    "kimi-k2.7",
    "kimi-k2.6",
    "deepseek-v4-pro",
];

/// 内置快照：上游不可达时至少不空手。
///
/// 数据来自参考实现（MIT）实测的 `STATIC_CN_MODELS`（39 条），只取
/// id / 倍率 / 上下文与输出上限 / 能力标记 —— 实时目录会逐字段覆盖它。
///
/// **倍率是会变的**：快照值只用于"实时目录没给时"的兜底，
/// 所以这里的条目一律标 `from_snapshot`，矩阵会把它记成 Builtin 来源。
const CN_SNAPSHOT_JSON: &str = r#"[
  {"id":"hy3","credits":"x0.00","maxInputTokens":192000,"maxOutputTokens":64000,"vendor":"j","reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"hy3-x","credits":"x0.05","maxInputTokens":192000,"maxOutputTokens":64000,"vendor":"j","reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"hy4-preview","credits":"x0.29","maxInputTokens":1000000,"maxOutputTokens":64000,"vendor":"j","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"hy4-preview-dev","credits":"x0.29","maxInputTokens":1000000,"maxOutputTokens":64000,"vendor":"j","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"hy4-preview-f","credits":"x0.00","maxInputTokens":1000000,"maxOutputTokens":64000,"vendor":"j","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"hy4-preview-x","credits":"x0.29","maxInputTokens":1000000,"maxOutputTokens":64000,"vendor":"j","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":false,"defaultEffort":"high","summary":"auto","supportedEfforts":["high"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.9,"top_p":1},
  {"id":"minimax-m2.5","credits":"x0.18","maxInputTokens":200000,"maxOutputTokens":48000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5v-turbo","credits":"x0.71","maxInputTokens":200000,"maxOutputTokens":64000,"vendor":"e","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5.3","credits":"x0.79","maxInputTokens":1000000,"maxOutputTokens":48000,"vendor":"e","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high","max"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5.3-flash","credits":"x0.06","maxInputTokens":1000000,"maxOutputTokens":32000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high","max"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5.2","credits":"x0.79","maxInputTokens":1000000,"maxOutputTokens":48000,"vendor":"e","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["high","xhigh"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5.1","credits":"x0.79","maxInputTokens":200000,"maxOutputTokens":48000,"vendor":"e","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-5.0-turbo","credits":"x0.95","maxInputTokens":200000,"maxOutputTokens":48000,"vendor":"e","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-4.6v","credits":"x0.11","maxInputTokens":128000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k3-1","credits":"x1.62","maxInputTokens":1000000,"maxOutputTokens":32000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high","xhigh"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k2.8-preview","credits":"x0.77","maxInputTokens":1000000,"maxOutputTokens":32000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["low","high","max"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k2.7","credits":"x0.57","maxInputTokens":256000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k2.6","credits":"x0.52","maxInputTokens":256000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k2.5","credits":"x0.45","maxInputTokens":256000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"kimi-k2-thinking","credits":"x0.54","maxInputTokens":256000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"minimax-m3","credits":"x0.25","maxInputTokens":512000,"maxOutputTokens":128000,"vendor":"f","contextWindow":{"supportedLengths":[300000,512000]},"reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"minimax-m2.7","credits":"x0.26","maxInputTokens":200000,"maxOutputTokens":48000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"glm-4.6","credits":"x0.23","maxInputTokens":168000,"maxOutputTokens":32000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"temperature":1},
  {"id":"deepseek-v4-flash","credits":"x0.17","maxInputTokens":1000000,"maxOutputTokens":50000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["high","xhigh"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":false,"temperature":1},
  {"id":"deepseek-v4.1-flash","credits":"x0.11","maxInputTokens":1000000,"maxOutputTokens":128000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"defaultEffort":"high","supportedEfforts":["low","high","max"],"summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"deepseek-v4-pro","credits":"x0.51","maxInputTokens":1000000,"maxOutputTokens":50000,"vendor":"f","contextWindow":{"supportedLengths":[300000,1000000]},"reasoning":{"canDisableThinking":true,"defaultEffort":"high","summary":"auto","supportedEfforts":["high","xhigh"]},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":false,"temperature":1},
  {"id":"deepseek-v3-2-volc","credits":"x0.29","maxInputTokens":96000,"maxOutputTokens":32000,"vendor":"f","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":1},
  {"id":"deepseek-v3-1-volc","credits":"x0.52","maxInputTokens":96000,"maxOutputTokens":32000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"temperature":0.8},
  {"id":"deepseek-v3-1-lkeap","credits":"x0.52","maxInputTokens":96000,"maxOutputTokens":32000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"temperature":0.8},
  {"id":"deepseek-v3-1","credits":"x0.52","maxInputTokens":96000,"maxOutputTokens":32000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"temperature":0.8},
  {"id":"deepseek-v3-0324-lkeap","credits":"x0.52","maxInputTokens":112000,"maxOutputTokens":16000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"temperature":0.8},
  {"id":"deepseek-r1-0528-lkeap","maxInputTokens":96000,"maxOutputTokens":16000,"vendor":"f","supportsImages":true,"supportsToolCall":true,"temperature":0.8},
  {"id":"kimi-k2-instruct-taiji","maxInputTokens":31000,"maxOutputTokens":8192,"vendor":"f","supportsImages":true,"supportsToolCall":true},
  {"id":"default-1.1","maxInputTokens":200000,"maxOutputTokens":8192,"vendor":"e","supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true},
  {"id":"deepseek-r1-0528","maxInputTokens":96000,"maxOutputTokens":8192,"vendor":"tencent"},
  {"id":"deepseek-v3-0324","maxInputTokens":96000,"maxOutputTokens":8192,"vendor":"tencent"},
  {"id":"default-1.2","maxInputTokens":200000,"maxOutputTokens":24000,"vendor":"e","supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true},
  {"id":"hunyuan-2.0-instruct","maxInputTokens":128000,"maxOutputTokens":16000,"vendor":"j","reasoning":{"effort":"medium","summary":"auto"},"supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,"onlyReasoning":true,"temperature":0.7,"top_p":0.8},
  {"id":"hunyuan-chat","maxInputTokens":128000,"maxOutputTokens":8192,"vendor":"j","supportsImages":true,"supportsToolCall":true}
]"#;

fn flag(item: &serde_json::Value, key: &str) -> bool {
    item.get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn text(item: &serde_json::Value, outer: &str, inner: &str) -> Option<String> {
    item.get(outer)?
        .get(inner)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn listed_u64(item: &serde_json::Value, outer: &str, inner: &str) -> Vec<u64> {
    item.get(outer)
        .and_then(|node| node.get(inner))
        .and_then(serde_json::Value::as_array)
        .map(|values| values.iter().filter_map(serde_json::Value::as_u64).collect())
        .unwrap_or_default()
}

fn listed_str(item: &serde_json::Value, inner: &str) -> Vec<String> {
    item.get("reasoning")
        .and_then(|reasoning| reasoning.get(inner))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn snapshot_cn() -> Vec<UpstreamModel> {
    let parsed: Vec<serde_json::Value> = serde_json::from_str(CN_SNAPSHOT_JSON).unwrap_or_default();
    parsed
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            Some(UpstreamModel {
                id,
                display_name: None,
                reasoning_effort_map: HashMap::new(),
                credits: item
                    .get("credits")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                context_length: item.get("maxInputTokens").and_then(serde_json::Value::as_u64),
                max_output_tokens: item
                    .get("maxOutputTokens")
                    .and_then(serde_json::Value::as_u64),
                vendor: item
                    .get("vendor")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                capabilities: ModelCapabilities {
                    vision: flag(&item, "supportsImages"),
                    tool_calls: flag(&item, "supportsToolCall"),
                    reasoning: flag(&item, "supportsReasoning"),
                    // 快照条目：三个位都来自我们自己的对照表，算"已声明"。
                    declared_tools: true,
                    declared_vision: true,
                },
                context_windows: listed_u64(&item, "contextWindow", "supportedLengths"),
                reasoning_efforts: listed_str(&item, "supportedEfforts"),
                reasoning_fixed_effort: text(&item, "reasoning", "effort"),
                reasoning_default_effort: text(&item, "reasoning", "defaultEffort"),
                reasoning_can_disable: item
                    .get("reasoning")
                    .and_then(|reasoning| reasoning.get("canDisableThinking"))
                    .and_then(serde_json::Value::as_bool),
                always_reasoning: flag(&item, "onlyReasoning"),
                temperature: item.get("temperature").and_then(serde_json::Value::as_f64),
                top_p: item.get("top_p").and_then(serde_json::Value::as_f64),
                from_snapshot: true,
                user_declared: false,
            })
        })
        .collect()
}

/// 解析 `/v3/config`：清单取 `agents[]`（`name == "cli"` 优先，否则取最长的一份），
/// 元数据递归找任意一层 `models: [{...带 id...}]`。返回 None = 拿不到清单（含空列表）。
pub fn parse_config(
    payload: &Value,
) -> Option<(Vec<String>, HashMap<String, Value>)> {
    let data = payload.get("data").unwrap_or(payload);

    let mut cli_lists: Vec<Vec<String>> = Vec::new();
    let mut other_lists: Vec<Vec<String>> = Vec::new();
    for agent in data
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(models) = agent.get("models").and_then(Value::as_array) else {
            continue;
        };
        let ids: Vec<String> = models
            .iter()
            .filter_map(|model| model.as_str().map(str::to_string))
            .filter(|id| !id.is_empty())
            .collect();
        if ids.is_empty() {
            continue;
        }
        if agent.get("name").and_then(Value::as_str) == Some("cli") {
            cli_lists.push(ids);
        } else {
            other_lists.push(ids);
        }
    }
    let pool = if cli_lists.is_empty() {
        other_lists
    } else {
        cli_lists
    };
    let best = pool.into_iter().max_by_key(Vec::len)?;

    // 保序去重。
    let mut seen = HashSet::new();
    let ids: Vec<String> = best.into_iter().filter(|id| seen.insert(id.clone())).collect();
    if ids.is_empty() {
        // 凭据不可用时上游给的就是这个：必须当失败。
        return None;
    }

    let mut meta = HashMap::new();
    walk_meta(payload, &mut meta);
    Some((ids, meta))
}

fn walk_meta(node: &Value, meta: &mut HashMap<String, Value>) {
    match node {
        Value::Object(map) => {
            if let Some(entries) = map.get("models").and_then(Value::as_array) {
                let looks_like_meta = entries
                    .first()
                    .is_some_and(|entry| entry.is_object() && entry.get("id").is_some());
                if looks_like_meta {
                    for entry in entries {
                        if let Some(id) = entry.get("id").and_then(Value::as_str) {
                            meta.entry(id.to_string()).or_insert_with(|| entry.clone());
                        }
                    }
                }
            }
            for value in map.values() {
                walk_meta(value, meta);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_meta(item, meta);
            }
        }
        _ => {}
    }
}

/// 实时清单裁剪：剔通道类、剔变体、免费兄弟优先。
pub fn curate(
    ids: &[String],
    meta: &HashMap<String, Value>,
    snapshot_credits: &HashMap<String, Option<String>>,
) -> Vec<String> {
    let credits_of = |id: &str| -> Option<String> {
        meta.get(id)
            .and_then(|entry| entry.get("credits"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| snapshot_credits.get(id).cloned().flatten())
    };
    // "认识"的模型 = 实时清单 ∪ 快照 ∪ 对外顺序表：只认快照会让
    // "实时才出现的免费兄弟"漏判，从而把该保留的免费档裁掉。
    let known = |id: &str| {
        ids.iter().any(|entry| entry == id)
            || snapshot_credits.contains_key(id)
            || CN_UI_ORDER.contains(&id)
    };
    let free = |id: &str| credits_is_free(credits_of(id).as_deref());

    let mut output = Vec::new();
    for id in ids {
        if !is_chat_model(id) {
            continue;
        }
        if VARIANT_SUFFIXES.iter().any(|suffix| id.ends_with(suffix)) {
            continue;
        }
        // X-f 免费而 X 收费 → 丢 X（保留免费的那个）。
        if let Some(base) = id.strip_suffix("-f") {
            if !base.is_empty() && known(base) && free(base) && !free(id) {
                continue;
            }
        } else {
            let sibling = format!("{id}-f");
            if known(&sibling) && free(&sibling) && !free(id) {
                continue;
            }
        }
        output.push(id.clone());
    }
    output
}

/// 合并成一个渠道目录：内置快照打底（过清洗），实时条目逐字段覆盖，实时独有的也上架。
pub fn merge(live_ids: &[String], meta: &HashMap<String, Value>, snapshot: &[UpstreamModel]) -> Vec<UpstreamModel> {
    let mut merged: Vec<UpstreamModel> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for entry in snapshot {
        if !is_chat_model(&entry.id) || index.contains_key(&entry.id) {
            continue;
        }
        // 只广告"对外顺序表里点名"的快照条目：历史模型与专线变体（-volc / -lkeap 等）
        // 默认不上架，等实时目录再次列出才出现 —— 否则客户端会 pin 到一个用不了的模型。
        if !CN_UI_ORDER.contains(&entry.id.as_str()) {
            continue;
        }
        index.insert(entry.id.clone(), merged.len());
        merged.push(entry.clone());
    }

    let mut live_only: Vec<String> = Vec::new();
    for id in live_ids {
        if !is_chat_model(id) {
            continue;
        }
        match index.get(id) {
            Some(position) => {
                overlay(&mut merged[*position], meta.get(id));
                // 实时目录确认了这条：不再是"仅在快照里"的条目。
                merged[*position].from_snapshot = false;
            }
            None => {
                let mut entry = UpstreamModel {
                    id: id.clone(),
                    ..UpstreamModel::default()
                };
                overlay(&mut entry, meta.get(id));
                index.insert(id.clone(), merged.len());
                merged.push(entry.clone());
                live_only.push(id.clone());
            }
        }
    }

    // 输出顺序：对外顺序表在前，实时独有的按上游顺序追加。
    let order_position = |id: &str| CN_UI_ORDER.iter().position(|known| *known == id);
    let live_position = |id: &str| live_ids.iter().position(|known| known == id).unwrap_or(usize::MAX);
    merged.sort_by_key(|entry| match order_position(&entry.id) {
        Some(position) => (0usize, position),
        None => (1, live_position(&entry.id)),
    });
    merged
}

/// 实时元数据逐字段覆盖快照条目。
fn overlay(entry: &mut UpstreamModel, meta: Option<&Value>) {
    let Some(meta) = meta else { return };
    if let Some(credits) = meta.get("credits").and_then(Value::as_str) {
        if !credits.is_empty() {
            entry.credits = Some(credits.to_string());
        }
    }
    if let Some(vendor) = meta.get("vendor").and_then(Value::as_str) {
        if !vendor.is_empty() {
            entry.vendor = Some(vendor.to_string());
        }
    }
    let positive = |value: Option<&Value>| value.and_then(Value::as_u64).filter(|value| *value > 0);
    let context = positive(meta.get("maxInputTokens")).or_else(|| {
        positive(
            meta.get("contextWindow")
                .and_then(|window| window.get("defaultLength")),
        )
    });
    if context.is_some() {
        entry.context_length = context;
    }
    if let Some(output) = positive(meta.get("maxOutputTokens")) {
        entry.max_output_tokens = Some(output);
    }
    let flag = |key: &str| meta.get(key).and_then(Value::as_bool);
    // 只有实时元数据里**真的带了**该键，才把对应的"已声明"置真；
    // 缺失时保留快照的判断（快照本身是已声明的）。
    let declared_vision = flag("supportsImages").is_some() || entry.capabilities.declared_vision;
    let declared_tools = flag("supportsToolCall").is_some() || entry.capabilities.declared_tools;
    entry.capabilities = ModelCapabilities {
        vision: flag("supportsImages").unwrap_or(entry.capabilities.vision),
        tool_calls: flag("supportsToolCall").unwrap_or(entry.capabilities.tool_calls),
        reasoning: flag("supportsReasoning").unwrap_or(entry.capabilities.reasoning),
        declared_tools,
        declared_vision,
    };
}

/// 拉实时目录。任何失败（网络 / 非 2xx / 空列表）都返回 None，由调用方回落快照。
pub async fn fetch_remote(
    client: &reqwest::Client,
    hosts: &Hosts,
    realm: identity::Realm,
    account: &AccountSnapshot,
) -> Option<(Vec<String>, HashMap<String, Value>)> {
    // 与参考实现一致：实时接口拿不到时先读桌面端自己的缓存文件，再退到内置快照。
    if let Some(cached) = read_desktop_cache(realm) {
        return Some(cached);
    }
    let mut headers = identity::with_user_agent(
        identity::base_headers(hosts),
        &identity::desktop_user_agent(realm),
    );
    headers.push((
        "Authorization".into(),
        format!(
            "Bearer {}",
            account.access_token.as_deref().unwrap_or_default()
        ),
    ));
    headers.push(("X-User-Id".into(), account.uid.clone()));

    let payload = transport::get_json(client, &hosts.chat_url(CONFIG_PATH), &headers)
        .await
        .ok()?;
    parse_config(&payload)
}

/// 读桌面端自己的目录缓存：`~/.workbuddy/cache/acc-product-config-v3.json`
/// （国际版是 `.workbuddy-ai`）。凭据失效或 WAF 拦了接口时，这是最完整的一份。
fn read_desktop_cache(realm: identity::Realm) -> Option<(Vec<String>, HashMap<String, Value>)> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let dir = match realm {
        identity::Realm::Cn => ".workbuddy",
        identity::Realm::Intl => ".workbuddy-ai",
    };
    let path = std::path::Path::new(&home)
        .join(dir)
        .join("cache")
        .join("acc-product-config-v3.json");
    let text = std::fs::read_to_string(path).ok()?;
    let payload: Value = serde_json::from_str(&text).ok()?;
    // 缓存文件的清单在顶层，接口响应在 data 下 —— parse_config 两种都收。
    parse_config(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot_index() -> HashMap<String, Option<String>> {
        snapshot_cn()
            .into_iter()
            .map(|entry| (entry.id, entry.credits))
            .collect()
    }

    #[test]
    fn non_chat_models_are_filtered_out() {
        for id in ["lite", "auto", "deep-model", "codewise-autocomplete", "completion-x", "hy3-image-alpha"] {
            assert!(!is_chat_model(id), "{id} 不该上架");
        }
        assert!(is_chat_model("glm-5.3"));
        assert!(!is_chat_model(""));
    }

    #[test]
    fn free_marker_only_accepts_the_known_zero_forms() {
        assert!(credits_is_free(Some("x0.00")));
        assert!(credits_is_free(Some("0")));
        assert!(credits_is_free(Some(" x0 ")));
        assert!(!credits_is_free(Some("x0.29")));
        assert!(!credits_is_free(None), "缺失不算免费");
        assert!(!credits_is_free(Some("free")));
    }

    #[test]
    fn cli_agent_list_wins_and_empty_list_is_a_failure() {
        let payload = json!({
            "code": 0,
            "data": {"agents": [
                {"name": "vscode", "models": ["a", "b", "c", "d"]},
                {"name": "cli", "models": ["glm-5.3", "hy3", "glm-5.3"]}
            ]}
        });
        let (ids, _) = parse_config(&payload).expect("有清单");
        assert_eq!(ids, vec!["glm-5.3", "hy3"], "cli 优先且保序去重");

        // 凭据不可用：HTTP 200 + 空列表 —— 必须当失败。
        assert!(parse_config(&json!({"code": 0, "data": {"agents": []}})).is_none());
        assert!(parse_config(&json!({"code": 0, "data": {}})).is_none());
    }

    #[test]
    fn metadata_is_found_at_any_depth() {
        let payload = json!({
            "data": {
                "agents": [{"name": "cli", "models": ["glm-5.3"]}],
                "catalog": {"nested": {"models": [
                    {"id": "glm-5.3", "credits": "x0.00", "maxInputTokens": 200000, "maxOutputTokens": 128000,
                     "supportsImages": true, "supportsToolCall": true, "supportsReasoning": true, "vendor": "zhipu"}
                ]}}
            }
        });
        let (_, meta) = parse_config(&payload).expect("有清单");
        let entry = meta.get("glm-5.3").expect("元数据");
        assert_eq!(entry["maxInputTokens"], 200000);
    }

    #[test]
    fn curate_drops_variants_and_prefers_free_siblings() {
        let ids: Vec<String> = ["deepseek-v4.1-flash", "deepseek-v4.1-flash-sg", "hy4-preview", "hy4-preview-f"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        let meta: HashMap<String, Value> = [(
            "hy4-preview".to_string(),
            json!({"id": "hy4-preview", "credits": "x0.29"}),
        )]
        .into_iter()
        .collect();
        let mut snapshot = snapshot_index();
        snapshot.insert("hy4-preview-f".into(), Some("x0.00".into()));
        let curated = curate(&ids, &meta, &snapshot);
        assert!(curated.contains(&"deepseek-v4.1-flash".to_string()));
        assert!(!curated.iter().any(|id| id.ends_with("-sg")), "变体必须剔除");
        assert!(!curated.contains(&"hy4-preview".to_string()), "免费兄弟在场时丢收费的");
        assert!(curated.contains(&"hy4-preview-f".to_string()));
    }

    #[test]
    fn merge_keeps_snapshot_order_and_appends_live_only_models() {
        let live: Vec<String> = ["brand-new-model", "hy3"].iter().map(|id| id.to_string()).collect();
        let meta: HashMap<String, Value> = [(
            "brand-new-model".to_string(),
            json!({"id": "brand-new-model", "credits": "x0.12", "maxOutputTokens": 8192}),
        )]
        .into_iter()
        .collect();
        let merged = merge(&live, &meta, &snapshot_cn());
        let ids: Vec<&str> = merged.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids[0], "hy4-preview-f", "顺序表在前");
        assert!(ids.contains(&"brand-new-model"), "实时独有的必须上架");
        let new_model = merged
            .iter()
            .find(|entry| entry.id == "brand-new-model")
            .expect("新模型");
        assert_eq!(new_model.credits.as_deref(), Some("x0.12"));
        assert_eq!(new_model.max_output_tokens, Some(8192));
    }

    #[test]
    fn live_metadata_overrides_snapshot_fields() {
        let live: Vec<String> = ["glm-5.3"].iter().map(|id| id.to_string()).collect();
        let meta: HashMap<String, Value> = [(
            "glm-5.3".to_string(),
            json!({"id": "glm-5.3", "credits": "x0.30", "maxInputTokens": 1000000,
                   "supportsImages": true}),
        )]
        .into_iter()
        .collect();
        let merged = merge(&live, &meta, &snapshot_cn());
        let entry = merged.iter().find(|entry| entry.id == "glm-5.3").expect("条目");
        assert_eq!(entry.credits.as_deref(), Some("x0.30"), "实时值覆盖快照");
        assert_eq!(entry.context_length, Some(1_000_000));
        assert!(entry.capabilities.vision);
    }

    #[test]
    fn snapshot_credits_match_the_reference_data() {
        let snapshot = snapshot_cn();
        let credits = |id: &str| {
            snapshot
                .iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.credits.clone())
        };
        // 这三个值是参考实现实测的：deepseek-v4.1-flash 与 glm-5.3 **不是**免费的，
        // 早期版本一度按文档样例把它们标成 x0.00 —— 那会让调度优先挑到付费模型。
        assert_eq!(credits("hy3").as_deref(), Some("x0.00"));
        assert_eq!(credits("hy4-preview-f").as_deref(), Some("x0.00"));
        assert_eq!(credits("deepseek-v4.1-flash").as_deref(), Some("x0.11"));
        assert_eq!(credits("glm-5.3").as_deref(), Some("x0.79"));
        assert_eq!(credits("deepseek-v4-pro").as_deref(), Some("x0.51"));
        assert!(
            !credits_is_free(Some("x0.11")),
            "x0.11 是付费档，不能被判成免费"
        );

        // 参数与思考档位也要跟着快照一起进来（否则面板只能显示"未知"）。
        let entry = snapshot
            .iter()
            .find(|entry| entry.id == "deepseek-v4.1-flash")
            .expect("快照里有这条");
        assert_eq!(entry.context_length, Some(1_000_000));
        assert_eq!(entry.max_output_tokens, Some(128_000));
        assert!(
            entry.reasoning_efforts.iter().any(|effort| effort == "high"),
            "思考档位来自快照：{:?}",
            entry.reasoning_efforts
        );
        // vendor 是上游自己的短代码（实测是 "f" 这类值），只断言"有值"。
        assert!(entry.vendor.is_some());

        let hy3 = snapshot
            .iter()
            .find(|entry| entry.id == "hy3")
            .expect("快照里有这条");
        assert_eq!(hy3.max_output_tokens, Some(64_000));
    }

    #[test]
    fn legacy_snapshot_entries_are_not_advertised_without_live_confirmation() {
        let snapshot = snapshot_cn();
        assert!(
            snapshot.iter().any(|entry| entry.id == "deepseek-v3-1-volc"),
            "快照里确实有专线变体"
        );
        // 实时目录只列了 glm-5.3：专线变体与历史模型都不该出现在对外清单里。
        let live: Vec<String> = vec!["glm-5.3".to_string()];
        let merged = merge(&live, &HashMap::new(), &snapshot);
        let ids: Vec<&str> = merged.iter().map(|entry| entry.id.as_str()).collect();
        assert!(ids.contains(&"glm-5.3"));
        assert!(!ids.contains(&"deepseek-v3-1-volc"), "{ids:?}");
        assert!(!ids.contains(&"hy3-x"), "{ids:?}");
        // 顺序表里的历史模型仍然上架（它们是 picker 会用的档位）。
        assert!(ids.contains(&"hy4-preview-f"), "{ids:?}");
    }

    #[test]
    fn user_agent_override_replaces_instead_of_appending() {
        let headers = identity::with_user_agent(
            vec![("User-Agent".into(), "WorkBuddy/5.5.6".into())],
            "WorkBuddy/5.5.6 WorkBuddy/5.5.6 CLI/2.137.1",
        );
        let agents: Vec<&String> = headers
            .iter()
            .filter(|(name, _)| name == "User-Agent")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(agents.len(), 1, "不能出现两个 User-Agent");
        assert!(agents[0].contains("CLI/2.137.1"));
    }
}
