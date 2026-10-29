//! 网关配置。
//!
//! 相对参考项目的变化：配置不再落 `config.json`，渠道与账号的唯一事实来源是
//! PostgreSQL（由 Go 管理面写入）。本模块只保留**运行时策略**（冷却、并发、
//! 亲和、打分），这些是网关自己的调参，不归管理面管。
//!
//! 渠道与账号由 [`crate::gateway::state::GatewayState`] 在启动时从 PG 加载。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 监听端口默认 18788（SPEC 4.1）。
///
/// 为什么是 18788 而不是 8788：8788 这个区间在本机已经被其他常驻服务占用
/// （pythonw 占 8788、参考项目占 8790），用它会必然触发端口回退，于是
/// `GATEWAY_BASE_URL` 与实际监听端口对不上，管理面会打到别人身上。
/// 18788 与它们同段但不重叠，回退扫描范围 18788..18798 也不会碰到 8790。
pub const DEFAULT_PORT: u16 = 18788;

/// 端口被占用时的回退扫描范围。
pub const PORT_SCAN_RANGE: u16 = 11;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 是否启用 HTTP 服务。关掉后只有内部接口可用（排障场景）。
    pub enabled: bool,
    pub port: u16,
    /// 是否允许局域网访问（绑 0.0.0.0）。默认只绑回环。
    pub allow_lan: bool,
    /// 下游 API Key 的兜底列表（正常走 PG 的 api_keys 表）。
    ///
    /// 保留这个字段是为了在数据库还没配好时能临时跑通链路；
    /// 非空时**只认这些 Key**，不再查库。
    pub api_keys: Vec<String>,
    pub affinity: AffinityConfig,
    pub cooldowns: CooldownConfig,
    pub concurrency: ConcurrencyConfig,
    pub scoring: ScoringConfig,
    /// 对外模型别名：`{"ds-flash": "deepseek-v4.1-flash"}`。
    pub aliases: HashMap<String, String>,
    /// 渠道目录缓存 TTL（秒）。
    pub catalog_ttl_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_PORT,
            allow_lan: false,
            api_keys: Vec::new(),
            affinity: AffinityConfig::default(),
            cooldowns: CooldownConfig::default(),
            concurrency: ConcurrencyConfig::default(),
            scoring: ScoringConfig::default(),
            aliases: HashMap::new(),
            catalog_ttl_seconds: 300,
        }
    }
}

/// 选号打分的权重：量纲统一在"权重 1.0 ≈ 一次完整缓存命中的偏好"附近。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoringConfig {
    /// 与上次使用的账号/渠道不同时的惩罚（换渠道必丢缓存）。
    pub switch_penalty: f64,
    /// 命中亲和绑定的加成。
    pub affinity_bonus: f64,
    /// 在飞负载的惩罚系数。
    pub load_weight: f64,
    /// 免费档的加成（配 0 关闭）。
    pub cost_bonus: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            switch_penalty: 0.35,
            affinity_bonus: 0.5,
            load_weight: 0.3,
            cost_bonus: 0.1,
        }
    }
}

/// 渠道的运行时配置：从 PG 的 `channels` 表加载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// 渠道实例 id（账号联合主键的一半）：如 `wb-cn`、`trae-cn`。
    pub id: String,
    /// 适配器类型（注册中心按它建实例）：如 `workbuddy`、`fake`。
    pub adapter: String,
    pub display_name: String,
    pub enabled: bool,
    /// 成本/质量档：越大越先被选中（免费档 > 付费档）。
    pub priority: u32,
    /// 同档内的容量份额。
    pub weight: u32,
    /// 适配器私有参数（域名、出站身份等）。
    pub options: serde_json::Value,
    /// 渠道级倍率（记账用，网关只透传给用量记录）。
    pub channel_ratio: f64,
    #[serde(default)]
    pub model_allow: Vec<String>,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            adapter: String::new(),
            display_name: String::new(),
            enabled: true,
            priority: 100,
            weight: 1,
            options: serde_json::Value::Null,
            channel_ratio: 1.0,
            model_allow: Vec::new(),
        }
    }
}

/// 一条「对外模型 → 渠道」的人工覆盖。
///
/// 由管理面写入 `channels`/`models` 之后，网关在构建矩阵时应用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RouteOverride {
    pub model: String,
    pub channel_id: String,
    /// 目录里有这条时用它改上游名；目录里没有时**必须**给，否则视为配置写错。
    pub upstream_model: Option<String>,
    pub priority: Option<u32>,
    pub weight: Option<u32>,
    pub enabled: Option<bool>,
}

/// 账号的运行时配置：从 PG 的 `accounts` 表加载。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountConfig {
    pub channel_id: String,
    pub uid: String,
    pub nickname: String,
    pub enabled: bool,
    pub priority_override: Option<u32>,
    pub weight_override: Option<u32>,
    pub max_inflight: Option<u32>,
    /// 该账号只服务哪些对外模型名（空 = 不限）。
    pub model_allow: Vec<String>,
    /// 账号级模型改名：`对外名 -> 该账号的上游名`。
    pub model_rename: HashMap<String, String>,
}

/// 会话亲和：绑定到账号级，保上游前缀缓存命中。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AffinityConfig {
    pub enabled: bool,
    /// prefer / strict / off
    pub mode: String,
    pub ttl_seconds: u64,
    pub max_entries: usize,
}

impl Default for AffinityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "prefer".to_string(),
            ttl_seconds: 1800,
            max_entries: 20_000,
        }
    }
}

/// 冷却与退避参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CooldownConfig {
    pub auth_secs: u64,
    pub soft_base_secs: u64,
    pub soft_cap_secs: u64,
    pub breaker_after: u32,
    pub breaker_base_secs: u64,
    pub breaker_cap_secs: u64,
}

impl Default for CooldownConfig {
    fn default() -> Self {
        Self {
            auth_secs: 60,
            soft_base_secs: 600,
            soft_cap_secs: 7_200,
            breaker_after: 3,
            breaker_base_secs: 1_800,
            breaker_cap_secs: 21_600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConcurrencyConfig {
    pub global: u32,
    pub per_account: u32,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            global: 32,
            per_account: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.port, DEFAULT_PORT);
        assert!(config.enabled);
        assert_eq!(config.concurrency.global, 32);
        assert_eq!(config.cooldowns.auth_secs, 60);
        assert!(config.affinity.enabled);
    }

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let config: Config = serde_json::from_str(r#"{"port": 8899}"#).expect("部分字段");
        assert_eq!(config.port, 8899);
        assert_eq!(config.concurrency.global, 32);
        assert!(config.enabled);
    }
}
