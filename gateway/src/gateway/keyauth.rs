//! API Key 鉴权与权限上下文（SPEC 第 12 节）。
//!
//! 参考实现只有 `api_keys: Vec<String>`：一串密钥白名单，所有 Key 共享同一张
//! 模型矩阵和同一个号池，没有任何隔离。结果是"一个跑 Cursor 的 Key 顺着矩阵
//! 落到 Trae 上烧额度，事后只能从用量记录倒推"。
//!
//! 这里给 Key 两个正交的白名单：
//!   · `channel_allow`：能用哪些渠道（成本边界）
//!   · `model_allow`：能用哪些对外模型（费用/质量边界）
//!
//! 两个都空 = 不限（保持升级前行为，SPEC 12.4）。

use std::collections::HashSet;

use crate::gateway::{
    errors::GatewayError,
    security::short_hash,
    state::SharedGateway,
    timeutil::now_ms,
};

/// 一次请求的 Key 权限上下文。
#[derive(Debug, Clone, Default)]
pub struct KeyContext {
    /// Key 的主键；未鉴权模式（本地免鉴权）为 None。
    pub key_id: Option<i64>,
    pub name: String,
    /// 允许的渠道实例 id；空 = 不限。
    pub channel_allow: HashSet<String>,
    /// 允许的对外模型名；空 = 不限。
    pub model_allow: HashSet<String>,
    /// 亲和作用域：用 Key 的短哈希，避免把密钥写进内存键与日志。
    pub scope: String,
}

impl KeyContext {
    /// 是否允许使用某个渠道。
    pub fn allows_channel(&self, channel_id: &str) -> bool {
        self.channel_allow.is_empty() || self.channel_allow.contains(channel_id)
    }

    /// 是否允许使用某个对外模型。
    pub fn allows_model(&self, model: &str) -> bool {
        self.model_allow.is_empty() || self.model_allow.contains(model)
    }

    /// 是否限制了渠道（用于决定错误消息的措辞）。
    pub fn restricts_channels(&self) -> bool {
        !self.channel_allow.is_empty()
    }
}

/// 未鉴权模式的上下文：两个白名单都空，等价于"不限"。
///
/// 只在 `api_keys` 为空（本地回环、免鉴权）时使用。
pub fn open_context() -> KeyContext {
    KeyContext {
        key_id: None,
        name: "local".into(),
        channel_allow: HashSet::new(),
        model_allow: HashSet::new(),
        scope: "open".into(),
    }
}

/// 解析请求里的 Key 并返回权限上下文。
///
/// 顺序（SPEC 7.3）：
///   1. 静态列表（`config.api_keys`）非空 → 只认它，不查库（排障兜底）；
///   2. 否则查 PG 的 `api_keys` 表；
///   3. 都为空 → 本地免鉴权模式。
pub async fn resolve(state: &SharedGateway, headers: &axum::http::HeaderMap) -> Result<KeyContext, GatewayError> {
    let provided = provided_key(headers);
    let static_keys = state.config().api_keys;

    // ① 静态列表优先：它是"数据库还没配好时"的应急通道。
    if !static_keys.is_empty() {
        let Some(key) = provided else {
            return Err(GatewayError::auth("缺少 API Key"));
        };

        let matched = static_keys
            .iter()
            .any(|candidate| crate::gateway::errors::secret_eq(candidate, &key));

        if !matched {
            return Err(GatewayError::auth("无效的 API Key"));
        }

        return Ok(KeyContext {
            key_id: None,
            name: "static".into(),
            channel_allow: HashSet::new(),
            model_allow: HashSet::new(),
            scope: short_hash(&key),
        });
    }

    // ② 查库。
    let Some(key) = provided else {
        // 没有任何 Key 配置 = 本地免鉴权（只绑回环时的便利）。
        // 一旦库里有了 Key，就必须带。
        if state.has_api_keys().await {
            return Err(GatewayError::auth("缺少 API Key"));
        }
        return Ok(open_context());
    };

    match state.lookup_api_key(&key).await {
        Ok(Some(record)) => {
            if !record.enabled {
                return Err(GatewayError::auth("该 API Key 已停用"));
            }
            if let Some(expires_at) = record.expires_at_ms {
                if expires_at <= now_ms() as i64 {
                    return Err(GatewayError::auth("该 API Key 已过期"));
                }
            }

            Ok(KeyContext {
                key_id: Some(record.id),
                name: record.name,
                channel_allow: record.channel_allow.into_iter().collect(),
                model_allow: record.model_allow.into_iter().collect(),
                scope: short_hash(&key),
            })
        }
        Ok(None) => {
            // 库里没有这个 Key：如果库里一条都没有，仍视为免鉴权模式。
            if state.has_api_keys().await {
                Err(GatewayError::auth("无效的 API Key"))
            } else {
                Ok(open_context())
            }
        }
        Err(error) => Err(error),
    }
}

/// 从请求头里取 Key：`Authorization: Bearer` 或 `x-api-key`。
///
/// 两个都收：OpenAI 客户端习惯用前者，Anthropic 客户端用后者。
pub fn provided_key(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .filter(|value| !value.is_empty())
}

/// 会话线索：不同客户端习惯不同，三个常见头都收（亲和键的第一级来源）。
pub fn header_session(headers: &axum::http::HeaderMap) -> Option<String> {
    ["x-conversation-id", "session-id", "x-session-id"]
        .iter()
        .find_map(|name| headers.get(*name))
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlists_mean_unrestricted() {
        let context = open_context();
        assert!(context.allows_channel("anything"));
        assert!(context.allows_model("anything"));
        assert!(!context.restricts_channels());
    }

    #[test]
    fn channel_allowlist_filters() {
        let context = KeyContext {
            channel_allow: ["wb-cn".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(context.allows_channel("wb-cn"));
        assert!(!context.allows_channel("trae-cn"));
        // 渠道受限不影响模型判定（两个维度正交）。
        assert!(context.allows_model("any-model"));
    }

    #[test]
    fn model_allowlist_filters() {
        let context = KeyContext {
            model_allow: ["glm-5.3".to_string()].into_iter().collect(),
            ..Default::default()
        };
        assert!(context.allows_model("glm-5.3"));
        assert!(!context.allows_model("deepseek-v4.1-flash"));
    }
}
