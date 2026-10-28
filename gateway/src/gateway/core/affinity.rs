//! 会话亲和：把同一会话钉在同一账号上，保住上游的前缀缓存命中。
//!
//! 这是调度里与"负载均衡"正面冲突的那一半，所以规则写得很明确：
//! 绑定粒度是**账号级**（缓存大概率按账号隔离）；客户端没给会话锚点时用
//! 内容前缀的链式哈希兜底（前缀没变就还能粘住）；失效是阶梯式的
//! （原账号 → 同渠道换号 → 跨渠道），每往下一级缓存必丢一次。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock,
    },
};

use sha2::{Digest, Sha256};

use crate::gateway::{config::AffinityConfig, protocol::cir::CanonicalRequest, timeutil::now_ms};

/// 内容指纹保留多少个断点位置（与 Anthropic/Bedrock "回看约 20 个 content block" 同构）。
const LOOKBACK_POSITIONS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityMode {
    /// 不做亲和。
    Off,
    /// 亲和优先，失败可换号（默认）。
    Prefer,
    /// 严格绑定：绑定不可用就直接失败，不换号重写缓存。
    Strict,
}

impl AffinityMode {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Self::Off,
            "strict" => Self::Strict,
            _ => Self::Prefer,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Prefer => "prefer",
            Self::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub channel_id: String,
    pub uid: String,
    pub upstream_model: String,
    pub expires_at_ms: u64,
    pub hits: u32,
}

pub struct AffinityStore {
    /// 0 = off / 1 = prefer / 2 = strict（原子存储，便于配置热更新时原地改）。
    mode: std::sync::atomic::AtomicU8,
    ttl_ms: AtomicU64,
    max_entries: std::sync::atomic::AtomicUsize,
    entries: RwLock<HashMap<String, Binding>>,
    hits: AtomicU64,
    lookups: AtomicU64,
}

impl AffinityStore {
    pub fn new(config: &AffinityConfig) -> Self {
        let store = Self {
            mode: std::sync::atomic::AtomicU8::new(0),
            ttl_ms: AtomicU64::new(0),
            max_entries: std::sync::atomic::AtomicUsize::new(0),
            entries: RwLock::new(HashMap::new()),
            hits: AtomicU64::new(0),
            lookups: AtomicU64::new(0),
        };
        store.reconfigure(config);
        store
    }

    /// 配置热更新：模式 / TTL / 容量原地生效，已有绑定保留。
    pub fn reconfigure(&self, config: &AffinityConfig) {
        let mode = if config.enabled {
            AffinityMode::parse(&config.mode)
        } else {
            AffinityMode::Off
        };
        let code = match mode {
            AffinityMode::Off => 0u8,
            AffinityMode::Prefer => 1,
            AffinityMode::Strict => 2,
        };
        self.mode.store(code, Ordering::Relaxed);
        self.ttl_ms.store(
            config.ttl_seconds.saturating_mul(1000).max(60_000),
            Ordering::Relaxed,
        );
        self.max_entries
            .store(config.max_entries.max(100), Ordering::Relaxed);
    }

    pub fn mode(&self) -> AffinityMode {
        match self.mode.load(Ordering::Relaxed) {
            0 => AffinityMode::Off,
            2 => AffinityMode::Strict,
            _ => AffinityMode::Prefer,
        }
    }

    pub fn enabled(&self) -> bool {
        self.mode() != AffinityMode::Off
    }

    fn ttl_ms(&self) -> u64 {
        self.ttl_ms.load(Ordering::Relaxed)
    }

    fn max_entries(&self) -> usize {
        self.max_entries.load(Ordering::Relaxed)
    }

    /// 候选键：显式会话锚点优先，其次是从最新前缀往回回溯的内容指纹。
    ///
    /// 顺序即优先级：lookup 会依次尝试，命中第一个可用的绑定。
    pub fn candidate_keys(
        scope: &str,
        public_model: &str,
        request: &CanonicalRequest,
    ) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(session) = request.metadata.session_key() {
            keys.push(format!("s:{scope}:{public_model}:{session}"));
        }
        let mut content = content_keys(scope, public_model, request);
        content.reverse(); // 最新（最长）的前缀优先
        keys.extend(content);
        keys
    }

    pub fn lookup(&self, keys: &[String], now: u64) -> Option<(String, Binding)> {
        if !self.enabled() || keys.is_empty() {
            return None;
        }
        self.lookups.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.write().ok()?;
        for key in keys {
            let Some(binding) = entries.get_mut(key) else {
                continue;
            };
            if binding.expires_at_ms <= now {
                entries.remove(key);
                continue;
            }
            // 命中即续期：长会话不该因为 TTL 到点而重新掷骰子。
            binding.expires_at_ms = now.saturating_add(self.ttl_ms());
            binding.hits = binding.hits.saturating_add(1);
            let hit = binding.clone();
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some((key.clone(), hit));
        }
        None
    }

    /// 写入/续期绑定，返回落库用的实际绑定（TTL 已补齐）；`None` 表示亲和被关闭。
    pub fn bind(&self, key: String, binding: Binding, now: u64) -> Option<Binding> {
        if !self.enabled() {
            return None;
        }
        let Ok(mut entries) = self.entries.write() else {
            return None;
        };
        if entries.len() >= self.max_entries() && !entries.contains_key(&key) {
            self.evict(&mut entries, now);
        }
        let stored = Binding {
            expires_at_ms: binding
                .expires_at_ms
                .max(now.saturating_add(self.ttl_ms())),
            ..binding
        };
        entries.insert(key, stored.clone());
        Some(stored)
    }

    pub fn clear(&self, key: &str) {
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(key);
        }
    }

    /// 清空某个账号上的全部绑定（账号被禁用/删除时调用，避免一直亲和到一个废账号）。
    pub fn clear_account(&self, channel_id: &str, uid: &str) -> usize {
        let Ok(mut entries) = self.entries.write() else {
            return 0;
        };
        let before = entries.len();
        entries.retain(|_, binding| !(binding.channel_id == channel_id && binding.uid == uid));
        before - entries.len()
    }

    pub fn clear_all(&self) -> usize {
        let Ok(mut entries) = self.entries.write() else {
            return 0;
        };
        let count = entries.len();
        entries.clear();
        count
    }

    /// 启动时从库里恢复绑定（进程重启不丢会话亲和）。
    pub fn restore(&self, rows: Vec<(String, Binding)>) {
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        let now = now_ms();
        for (key, binding) in rows {
            if binding.expires_at_ms > now {
                entries.insert(key, binding);
            }
        }
    }

    pub fn stats(&self) -> serde_json::Value {
        let count = self.entries.read().map(|entries| entries.len()).unwrap_or(0);
        let hits = self.hits.load(Ordering::Relaxed);
        let lookups = self.lookups.load(Ordering::Relaxed);
        let rate = if lookups > 0 {
            hits as f64 / lookups as f64
        } else {
            0.0
        };
        serde_json::json!({
            "mode": self.mode().as_str(),
            "entries": count,
            "capacity": self.max_entries(),
            "ttl_seconds": self.ttl_ms() / 1000,
            "lookups": lookups,
            "hits": hits,
            "hit_rate": (rate * 1000.0).round() / 1000.0,
        })
    }

    /// 当前绑定快照：面板"调度"视图与排障用（谁被钉在哪个账号上）。
    pub fn snapshot(&self) -> Vec<(String, Binding)> {
        self.entries
            .read()
            .map(|entries| {
                entries
                    .iter()
                    .map(|(key, binding)| (key.clone(), binding.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn evict(&self, entries: &mut HashMap<String, Binding>, now: u64) {
        entries.retain(|_, binding| binding.expires_at_ms > now);
        if entries.len() < self.max_entries() {
            return;
        }
        // 还满就按最久未用（expires_at 最小）砍掉一批。
        let mut deadlines: Vec<(u64, String)> = entries
            .iter()
            .map(|(key, binding)| (binding.expires_at_ms, key.clone()))
            .collect();
        deadlines.sort();
        for (_, key) in deadlines
            .into_iter()
            .take(entries.len() / 10 + 1)
        {
            entries.remove(&key);
        }
    }
}

/// 链式内容指纹：对 tools + 各消息文本块逐块累积哈希，每个块边界留一个键。
///
/// 追加消息不会改变已有块的哈希，所以"会话续写"仍能命中旧绑定 —— 这是整串哈希做不到的。
fn content_keys(scope: &str, public_model: &str, request: &CanonicalRequest) -> Vec<String> {
    let mut digest = Sha256::new();
    digest.update(scope.as_bytes());
    digest.update(b"|");
    digest.update(public_model.as_bytes());
    for tool in &request.tools {
        digest.update(b"|tool:");
        digest.update(tool.name.as_bytes());
    }
    for block in &request.system {
        if let Some(text) = block.as_text() {
            digest.update(b"|sys:");
            digest.update(text.as_bytes());
        }
    }

    let mut keys = Vec::new();
    for message in &request.messages {
        for block in &message.content {
            let Some(text) = block.as_text() else {
                continue;
            };
            digest.update(b"|m:");
            digest.update(text.as_bytes());
            let hash = digest.clone().finalize();
            keys.push(format!("c:{scope}:{public_model}:{}", hex(&hash)));
        }
    }
    if keys.len() > LOOKBACK_POSITIONS {
        keys.split_off(keys.len() - LOOKBACK_POSITIONS)
    } else {
        keys
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text.truncate(24);
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::cir::{Block, Message, Role};

    fn request(session: Option<&str>, texts: &[&str]) -> CanonicalRequest {
        CanonicalRequest {
            model: "glm-5.3".into(),
            system: vec![Block::text("你是助手")],
            messages: texts
                .iter()
                .map(|text| Message {
                    role: Role::User,
                    content: vec![Block::text(*text)],
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    reasoning: None,
                })
                .collect(),
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning: None,
            metadata: crate::gateway::protocol::cir::RequestMetadata {
                header_session: session.map(str::to_string),
                ..Default::default()
            },
            raw: serde_json::json!({}),
        }
    }

    fn store() -> AffinityStore {
        AffinityStore::new(&AffinityConfig::default())
    }

    fn binding(channel: &str, uid: &str, model: &str) -> Binding {
        Binding {
            channel_id: channel.into(),
            uid: uid.into(),
            upstream_model: model.into(),
            expires_at_ms: 0,
            hits: 0,
        }
    }

    #[test]
    fn explicit_session_key_is_tried_first_and_hits() {
        let store = store();
        let keys = AffinityStore::candidate_keys("open", "glm-5.3", &request(Some("conv-1"), &["你好"]));
        assert!(keys[0].starts_with("s:open:glm-5.3:conv-1"));
        let now = now_ms();
        store.bind(keys[0].clone(), binding("wb-cn", "u1", "glm-5.3"), now);

        let (key, hit) = store.lookup(&keys, now).expect("应当命中");
        assert_eq!(key, keys[0]);
        assert_eq!(hit.uid, "u1");
        assert_eq!(hit.hits, 1, "命中要续期并计数");
    }

    #[test]
    fn appended_messages_still_hit_the_earlier_prefix() {
        let store = store();
        let now = now_ms();
        let first = AffinityStore::candidate_keys("open", "glm-5.3", &request(None, &["第一轮"]));
        // 把绑定挂在"第一轮"这个前缀位置上。
        let prefix_key = first.last().expect("有内容键").clone();
        store.bind(prefix_key.clone(), binding("wb-cn", "u1", "glm-5.3"), now);

        // 会话续写：多了一条消息，旧前缀键仍然在候选里，所以还能命中。
        let continued = AffinityStore::candidate_keys(
            "open",
            "glm-5.3",
            &request(None, &["第一轮", "第二轮"]),
        );
        let (key, _) = store.lookup(&continued, now).expect("续写仍应命中旧绑定");
        assert_eq!(key, prefix_key);
    }

    #[test]
    fn different_scope_or_model_never_shares_a_binding() {
        let store = store();
        let now = now_ms();
        let keys = AffinityStore::candidate_keys("key-a", "glm-5.3", &request(Some("conv-1"), &["你好"]));
        store.bind(keys[0].clone(), binding("wb-cn", "u1", "glm-5.3"), now);

        let other_scope = AffinityStore::candidate_keys("key-b", "glm-5.3", &request(Some("conv-1"), &["你好"]));
        assert!(store.lookup(&other_scope, now).is_none(), "不同 API Key 不该共享绑定");

        let other_model = AffinityStore::candidate_keys("key-a", "hy3", &request(Some("conv-1"), &["你好"]));
        assert!(store.lookup(&other_model, now).is_none(), "不同模型不该共享绑定");
    }

    #[test]
    fn expired_binding_is_dropped_on_lookup() {
        let store = AffinityStore::new(&AffinityConfig {
            ttl_seconds: 60,
            ..AffinityConfig::default()
        });
        let keys =
            AffinityStore::candidate_keys("open", "glm-5.3", &request(Some("conv-1"), &["你好"]));
        let now = now_ms();
        store.bind(keys[0].clone(), binding("wb-cn", "u1", "glm-5.3"), now);
        assert!(store.lookup(&keys, now).is_some(), "刚绑定应当命中");

        // TTL 之后再看：绑定过期并被就地清掉（不会一直亲和到一个可能已失效的账号）。
        let later = now + 61_000;
        assert!(store.lookup(&keys, later).is_none());
        assert!(store.snapshot().is_empty(), "过期条目要被清理");
    }

    #[test]
    fn off_mode_neither_binds_nor_looks_up() {
        let store = AffinityStore::new(&AffinityConfig {
            enabled: false,
            ..AffinityConfig::default()
        });
        let keys = AffinityStore::candidate_keys("open", "glm-5.3", &request(Some("conv-1"), &["你好"]));
        store.bind(keys[0].clone(), binding("wb-cn", "u1", "glm-5.3"), now_ms());
        assert!(store.lookup(&keys, now_ms()).is_none());
        assert_eq!(store.stats()["mode"], "off");
    }

    #[test]
    fn clear_account_removes_only_that_accounts_bindings() {
        let store = store();
        let now = now_ms();
        store.bind("k1".into(), binding("wb-cn", "u1", "glm-5.3"), now);
        store.bind("k2".into(), binding("wb-cn", "u2", "glm-5.3"), now);
        assert_eq!(store.clear_account("wb-cn", "u1"), 1);
        assert_eq!(store.snapshot().len(), 1);
        assert_eq!(store.clear_all(), 1);
        assert!(store.snapshot().is_empty());
    }
}
