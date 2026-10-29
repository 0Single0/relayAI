//! 账号池：按渠道分片的账号注册表 + 环形轮询挑选。
//!
//! 池子本身是"哑容器"：账号由 `state` 组装（它同时知道配置与渠道适配器），
//! 池子只负责存放、挑选与冷却策略。

pub mod account;

use std::sync::{Arc, RwLock};

use crate::gateway::{config::CooldownConfig, timeutil::now_ms};

pub use account::{humanize, AccountKey, AccountView, InflightGuard, PoolAccount};

pub struct AccountPool {
    accounts: RwLock<Vec<Arc<PoolAccount>>>,
    policy: CooldownConfig,
}

impl AccountPool {
    pub fn new(accounts: Vec<Arc<PoolAccount>>, policy: CooldownConfig) -> Self {
        Self {
            accounts: RwLock::new(accounts),
            policy,
        }
    }

    pub fn accounts(&self) -> Vec<Arc<PoolAccount>> {
        self.accounts
            .read()
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    pub fn policy(&self) -> CooldownConfig {
        self.policy.clone()
    }

    pub fn by_channel(&self, channel_id: &str) -> Vec<Arc<PoolAccount>> {
        self.accounts()
            .into_iter()
            .filter(|account| account.key.channel_id == channel_id)
            .collect()
    }

    pub fn get(&self, key: &AccountKey) -> Option<Arc<PoolAccount>> {
        self.accounts()
            .into_iter()
            .find(|account| account.key == *key)
    }

    /// 替换全部账号（面板增删账号后调用）。
    ///
    /// 同一个 key 的账号会**继承旧的运行态**（额度/冷却/熔断/失败计数）：
    /// 否则"停用某个账号"这类操作会顺手把全池的冷却清掉、额度清空。
    pub fn replace(&self, accounts: Vec<Arc<PoolAccount>>) {
        let Ok(mut slot) = self.accounts.write() else {
            return;
        };
        let previous: std::collections::HashMap<AccountKey, Arc<PoolAccount>> = slot
            .iter()
            .map(|account| (account.key.clone(), Arc::clone(account)))
            .collect();
        for account in &accounts {
            if let Some(old) = previous.get(&account.key) {
                account.carry_runtime_from(old);
            }
        }
        *slot = accounts;
    }

    pub fn views(&self) -> Vec<AccountView> {
        let now = now_ms();
        self.accounts()
            .iter()
            .map(|account| account.view(now))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::errors::GatewayError;

    fn pool_with(accounts: &[(&str, &str)]) -> AccountPool {
        let built = accounts
            .iter()
            .map(|(channel, uid)| {
                let mut account = PoolAccount::new(*channel, *uid);
                account.nickname = (*uid).to_string();
                account.requires_credential = false;
                account.set_max_inflight(1);
                Arc::new(account)
            })
            .collect();
        AccountPool::new(built, CooldownConfig::default())
    }

    #[test]
    fn rebuild_keeps_cooldowns_and_quota() {
        let pool = pool_with(&[("fake", "u1")]);
        let first = pool.accounts()[0].clone();
        first.apply_error(
            &GatewayError::new(
                crate::gateway::errors::ErrorKind::AuthExpired,
                crate::gateway::errors::ErrorScope::Account,
                401,
                "x",
                true,
            ),
            &CooldownConfig::default(),
            now_ms(),
        );
        first.set_quota(Some(crate::gateway::channels::QuotaSnapshot {
            label: "积分".into(),
            unit: "credits".into(),
            remaining: Some(42.0),
            total: Some(100.0),
            ..Default::default()
        }));
        assert!(!first.is_ready("m", now_ms()));

        // 模拟面板改账号：用同一批 key 重建一次
        pool.replace(pool_with(&[("fake", "u1")]).accounts());

        let rebuilt = pool.accounts()[0].clone();
        assert!(
            !rebuilt.is_ready("m", now_ms()),
            "冷却必须跟着重建存活，否则编辑账号就等于关掉保护"
        );
        assert_eq!(
            rebuilt.quota().map(|quota| quota.remaining),
            Some(Some(42.0)),
            "额度要继承，否则界面上会突然变成空"
        );
        assert_eq!(rebuilt.quota().map(|quota| quota.label), Some("积分".to_string()));
    }

    #[test]
    fn rebuild_applies_changed_concurrency_and_default() {
        let pool = pool_with(&[("fake", "u1")]);
        pool.accounts()[0].set_max_inflight(3);
        let changed = pool_with(&[("fake", "u1")]);
        changed.accounts()[0].set_max_inflight(1111);
        pool.replace(changed.accounts());
        assert_eq!(pool.accounts()[0].max_inflight(), 1111);
        let defaulted = pool_with(&[("fake", "u1")]);
        defaulted.accounts()[0].set_max_inflight(3);
        pool.replace(defaulted.accounts());
        assert_eq!(pool.accounts()[0].max_inflight(), 3);
    }

    #[test]
    fn accounts_can_be_listed_by_channel_and_replaced() {
        let pool = pool_with(&[("wb-cn", "u1"), ("qoder", "u2")]);
        assert_eq!(pool.by_channel("wb-cn").len(), 1);
        assert_eq!(pool.by_channel("qoder").len(), 1);
        assert_eq!(pool.by_channel("nope").len(), 0);

        let replacement = pool_with(&[("wb-cn", "u9")]);
        pool.replace(replacement.accounts());
        assert_eq!(pool.accounts().len(), 1);
        assert_eq!(pool.accounts()[0].key.uid, "u9");
    }
}
