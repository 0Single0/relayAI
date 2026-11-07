//! 账号运行态、凭据与冷却策略。
//!
//! 运行态全在内存（重启即清空），可用性永远是**实时计算值**而不是落库状态。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, RwLock,
    },
};

use crate::gateway::{
    channels::{AccountSnapshot, Credential, QuotaSnapshot},
    config::CooldownConfig,
    errors::{ErrorKind, ErrorScope, GatewayError},
    timeutil::next_local_4am_ms,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountKey {
    pub channel_id: String,
    pub uid: String,
}

impl AccountKey {
    pub fn new(channel_id: impl Into<String>, uid: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            uid: uid.into(),
        }
    }
}

impl std::fmt::Display for AccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.channel_id, self.uid)
    }
}

#[derive(Debug, Default)]
pub struct AccountRuntime {
    credentials: RwLock<Credential>,
    quota: RwLock<Option<QuotaSnapshot>>,
    max_inflight: AtomicU32,
    /// 账号级冷却（401 / 无 reset 的 429）。
    cooldown_until: AtomicU64,
    /// 402 余额停放，到次日本地 04:00。
    balance_until: AtomicU64,
    /// 连续 5xx 熔断。
    breaker_until: AtomicU64,
    /// 连续失败计数（5xx 与网络抖动共用），成功清零。
    fails: AtomicU32,
    /// 账号级软限流连续次数，决定退避倍数。
    soft_streak: AtomicU32,
    /// 在飞请求数。
    inflight: AtomicU32,
    /// 模型级冷却（key = **上游**模型名）：同账号其他模型照用。
    model_cooldowns: RwLock<HashMap<String, u64>>,
    /// 模型级软限流连续次数（上游没给恢复时刻时，按模型各自退避）。
    model_streaks: RwLock<HashMap<String, u32>>,
    /// 刷新串行锁：上游会轮换 refresh_token，并发刷新互相作废。
    pub refresh_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug)]
pub struct PoolAccount {
    pub key: AccountKey,
    pub nickname: String,
    pub enabled: bool,
    pub membership_enabled: bool,
    /// 该渠道是否必须有凭据才可用（假渠道不需要）。
    pub requires_credential: bool,
    /// 凭据是静态密钥（自定义供应商）还是会过期的令牌。静态凭据不参与刷新。
    pub static_credential: bool,
    /// 越大越先被选中：成本档（免费 > 付费）。
    pub priority: u32,
    /// 同档内的容量份额。
    pub weight: u32,
    /// 账号级覆盖值（`None` = 跟随渠道）。上面两个是**生效值**，这两个是来源，
    /// 面板编辑账号时要能区分"我自己设的 100"和"跟着渠道的 100"。
    pub priority_override: Option<u32>,
    pub weight_override: Option<u32>,
    /// 并发上限（面板可调）。原子：PoolAccount 常在 Arc 后面共享，&self 也要能改。
    pub proxy_slot: Option<String>,
    /// 这个账号只服务哪些**对外模型名**。空 = 不限制。
    ///
    /// 用途：同一渠道里有的号跑不了某些模型（没开通 / 单独计费），或想让某个号专跑某几个模型。
    /// 过滤发生在候选展开（`router::candidates`），被挡下的模型根本不会被选到。
    pub model_allow: Vec<String>,
    /// 账号级模型改名：`对外名 -> 该账号的上游名`。
    ///
    /// 与渠道级 `config.routes[].upstream_model` 的区别是**作用域**：同一渠道的两个账号
    /// 可以把同一个对外模型映射到不同的上游名。Trae 把 `deepseek-v4.1-flash` 叫
    /// `DeepSeek-V4-Flash-Official`，配一条改名就能让客户端继续用统一的名字。
    pub model_rename: std::collections::BTreeMap<String, String>,
    /// 该账号的上游基址（空 = 用适配器内置默认）。
    ///
    /// 放在账号上而不是渠道上：渠道是计价层，一份计价策略下可能有多个不同
    /// 地址的账号（自建中转、官方直连、备用反代）。
    pub base_url: String,
    /// 该账号的上游协议：`openai-chat` / `openai-responses` / `anthropic`。
    pub protocol: String,
    /// 该账号的平台（`workbuddy` / `trae` / `custom` / `fake`）。
    ///
    /// 平台是**账号属性**：一个计价渠道下面可以混不同平台的号，
    /// 适配器按账号选实例，而不是按渠道。
    pub platform: String,
    /// 适配器私有参数（workbuddy 的 realm / outbound_identity 等）。
    pub options: serde_json::Value,
    /// 出站代理（`None` = 直连）。
    ///
    /// 上游按出口 IP 限流：所有账号共用一个 IP 时，几十个号叠在一起
    /// 很容易被判定成异常流量。按账号绑不同代理可以把风险摊开。
    pub proxy: Option<crate::gateway::storage::ProxyRecord>,
    pub runtime: Arc<AccountRuntime>,
}

impl PoolAccount {
    pub fn new(channel_id: impl Into<String>, uid: impl Into<String>) -> Self {
        Self {
            key: AccountKey::new(channel_id, uid),
            nickname: String::new(),
            enabled: true,
            requires_credential: true,
            static_credential: false,
            priority: 100,
            weight: 1,
            priority_override: None,
            weight_override: None,
            proxy_slot: None,
            model_allow: Vec::new(),
            model_rename: std::collections::BTreeMap::new(),
            base_url: String::new(),
            protocol: String::new(),
            platform: String::new(),
            options: serde_json::Value::Null,
            proxy: None,
            runtime: Arc::new(AccountRuntime {
                credentials: RwLock::new(Credential::default()),
                quota: RwLock::new(None),
                max_inflight: AtomicU32::new(1),
                ..AccountRuntime::default()
            }),
            membership_enabled: true,
        }
    }

    pub fn snapshot(&self) -> AccountSnapshot {
        let credentials = self.credentials();
        AccountSnapshot {
            channel_id: self.key.channel_id.clone(),
            uid: self.key.uid.clone(),
            base_url: (!self.base_url.is_empty()).then(|| self.base_url.clone()),
            protocol: (!self.protocol.is_empty()).then(|| self.protocol.clone()),
            platform: (!self.platform.is_empty()).then(|| self.platform.clone()),
            options: (!self.options.is_null()).then(|| self.options.clone()),
            access_token: (!credentials.access_token.is_empty())
                .then(|| credentials.access_token.clone()),
            refresh_token: credentials.refresh_token.clone(),
            enterprise_id: credentials.enterprise_id.clone(),
            access_key_id: credentials.access_key_id.clone(),
            secret_key: credentials.secret_key.clone(),
            code_verifier: credentials.code_verifier.clone(),
        }
    }

    pub fn credentials(&self) -> Credential {
        self.runtime.credentials
            .read()
            .map(|credentials| credentials.clone())
            .unwrap_or_default()
    }

    pub fn quota(&self) -> Option<QuotaSnapshot> {
        self.runtime.quota
            .read()
            .ok()
            .and_then(|quota| quota.clone())
    }

    pub fn set_quota(&self, quota: Option<QuotaSnapshot>) {
        if let Ok(mut slot) = self.runtime.quota.write() {
            *slot = quota;
        }
    }

    /// 落库/刷新成功后整体替换凭据（refresh_token 会轮换，不能只换一半）。
    pub fn set_credentials(&self, credential: Credential) {
        if let Ok(mut slot) = self.runtime.credentials.write() {
            *slot = credential;
        }
    }

    /// 实时可用性：启用 ∧ 有凭据（若该渠道需要）∧ 不在冷却/停放/熔断窗口 ∧ 该模型没被单独冷掉。
    pub fn is_ready(&self, model: &str, now: u64) -> bool {
        if !self.enabled || !self.membership_enabled {
            return false;
        }
        // 用 `is_present()` 而不是只看 access_token：CodeArts 的长期 AK/SK 没有 token。
        if self.requires_credential && !self.credentials().is_present() {
            return false;
        }
        let blocked = self.runtime.cooldown_until.load(Ordering::SeqCst)
            | self.runtime.balance_until.load(Ordering::SeqCst)
            | self.runtime.breaker_until.load(Ordering::SeqCst);
        if now < blocked {
            return false;
        }
        if self
            .model_cooldowns()
            .get(model)
            .is_some_and(|until| now < *until)
        {
            return false;
        }
        true
    }

    /// 仅网络/5xx 熔断时保留会话归属；不覆盖缺凭据、限流、额度或启停状态。
    pub fn affinity_recovering(&self, model: &str, now: u64) -> bool {
        self.runtime_breaker_after(now).is_some()
            && self.enabled
            && self.membership_enabled
            && (!self.requires_credential || self.credentials().is_present())
            && self.runtime_cooldown_after(now).is_none()
            && self.runtime_balance_after(now).is_none()
            && !self
                .model_cooldowns()
                .get(model)
                .is_some_and(|until| now < *until)
    }

    /// 取一个在飞槽位。返回 None = 该账号并发已满（调度器应换号或让请求排队）。
    pub fn try_acquire(self: &Arc<Self>) -> Option<InflightGuard> {
        if self.runtime.inflight.load(Ordering::SeqCst) >= self.max_inflight() {
            return None;
        }
        // 先加再看：避免并发下两个请求同时通过上限判断。
        if self.runtime.inflight.fetch_add(1, Ordering::SeqCst) >= self.max_inflight() {
            self.runtime.inflight.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(InflightGuard {
            account: Arc::clone(self),
        })
    }

    /// 按统一语义落地冷却。策略参数见计划 §6.4，全部来自配置。
    pub fn apply_error(&self, error: &GatewayError, policy: &CooldownConfig, now: u64) {
        match error.kind {
            ErrorKind::AuthExpired => {
                self.raise_cooldown(&self.runtime.cooldown_until, now, policy.auth_secs);
            }
            ErrorKind::RateLimited => {
                // 三种形态，区别只在**范围**与**停多久**：
                // · 渠道说是模型级（AccountModel）：只停那个模型，同账号其他模型照用 ——
                //   上游给了恢复时刻就停到那一刻，没给就按模型各自的退避（不牵连账号）；
                // · 账号级但给了恢复时刻：停整个账号到那一刻；
                // · 账号级没给时刻：账号级指数退避（软限流，最保守）。
                let model = match &error.scope {
                    ErrorScope::AccountModel(model) => Some(model.clone()),
                    _ => None,
                };
                match (model, error.reset_at_ms) {
                    (Some(model), Some(reset_at)) => self.park_model(&model, reset_at),
                    (Some(model), None) => {
                        let streak = self.bump_model_streak(&model);
                        let wait = backoff_secs(
                            policy.soft_base_secs,
                            streak - 1,
                            policy.soft_cap_secs,
                        );
                        self.park_model(&model, now.saturating_add(wait * 1000));
                    }
                    (None, Some(reset_at)) => {
                        // 上游说了恢复时刻但没说哪个模型：按它停整个账号。
                        if reset_at > self.runtime.cooldown_until.load(Ordering::SeqCst) {
                            self.runtime.cooldown_until.store(reset_at, Ordering::SeqCst);
                        }
                    }
                    (None, None) => {
                        let streak = self.runtime.soft_streak.fetch_add(1, Ordering::SeqCst) + 1;
                        let wait =
                            backoff_secs(policy.soft_base_secs, streak - 1, policy.soft_cap_secs);
                        self.raise_cooldown(&self.runtime.cooldown_until, now, wait);
                    }
                }
            }
            // 渠道校验拦截与模型容量饱和都不是账号健康问题，不累积账号冷却或熔断。
            ErrorKind::ChannelBlocked | ErrorKind::ModelBusy => {}
            ErrorKind::QuotaExhausted => {
                self.runtime
                    .balance_until
                    .store(next_local_4am_ms(now), Ordering::SeqCst);
            }
            ErrorKind::Upstream5xx | ErrorKind::Network => {
                let fails = self.runtime.fails.fetch_add(1, Ordering::SeqCst) + 1;
                if fails >= policy.breaker_after {
                    let wait = backoff_secs(
                        policy.breaker_base_secs,
                        fails - policy.breaker_after,
                        policy.breaker_cap_secs,
                    );
                    let until = now.saturating_add(wait * 1000);
                    if until > self.runtime.breaker_until.load(Ordering::SeqCst) {
                        self.runtime.breaker_until.store(until, Ordering::SeqCst);
                    }
                }
            }
            // 内容审核换号也没用（原号降级重试在提示词管道里做）；
            // 请求类错误不改账号状态。
            // KeyForbidden 同理：那是**调用方权限**问题，与账号健康无关，
            // 绝不能因为某个 Key 越权访问就把账号冷却掉。
            ErrorKind::ContentPolicy
            // 只记录上游的限制结论；没有恢复时刻，不推断永久停用或自动解封。
            | ErrorKind::AccountRestricted
            | ErrorKind::InvalidRequest
            | ErrorKind::ModelNotFound
            | ErrorKind::NoAccount
            | ErrorKind::Unsupported
            | ErrorKind::KeyForbidden
            | ErrorKind::Internal => {}
        }
    }

    /// 重建账号对象时把**运行态**搬过来：额度、冷却、熔断、失败计数。
    ///
    /// 为什么必须搬：面板改账号（启停、改优先级）走的是"重建整池"，
    /// 不搬就等于每次编辑都把全池的冷却和熔断清空一次 —— 保护机制形同虚设，
    /// 额度也会显示成空。
    ///
    /// 在飞数**不搬**：在途请求的守卫持有的是旧对象，搬过来只会让计数永久偏高。
    pub fn carry_runtime_from(&self, previous: &PoolAccount) {
        let load = |slot: &AtomicU64| slot.load(Ordering::SeqCst);
        self.runtime
            .cooldown_until
            .store(load(&previous.runtime.cooldown_until), Ordering::SeqCst);
        self.runtime
            .balance_until
            .store(load(&previous.runtime.balance_until), Ordering::SeqCst);
        self.runtime
            .breaker_until
            .store(load(&previous.runtime.breaker_until), Ordering::SeqCst);
        self.runtime
            .fails
            .store(previous.runtime.fails.load(Ordering::SeqCst), Ordering::SeqCst);
        self.runtime.soft_streak.store(
            previous.runtime.soft_streak.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if let (Ok(source), Ok(mut target)) = (
            previous.runtime.model_cooldowns.read(),
            self.runtime.model_cooldowns.write(),
        ) {
            *target = source.clone();
        }
        if let (Ok(source), Ok(mut target)) = (
            previous.runtime.model_streaks.read(),
            self.runtime.model_streaks.write(),
        ) {
            *target = source.clone();
        }
        if let Some(quota) = previous.quota() {
            self.set_quota(Some(quota));
        }
    }

    /// 清空全部惩罚状态（面板"重置状态"按钮）。
    ///
    /// 只清运行态：凭据、额度、启停都不动。
    /// 调整并发上限（面板可改）。不低于 1；在飞中的请求不受影响（只影响新请求）。
    pub fn set_max_inflight(&self, value: u32) {
        self.runtime.max_inflight.store(value.max(1), Ordering::SeqCst);
    }

    pub fn max_inflight(&self) -> u32 {
        self.runtime.max_inflight.load(Ordering::SeqCst)
    }

    pub fn reset_runtime(&self) {
        self.runtime.cooldown_until.store(0, Ordering::SeqCst);
        self.runtime.balance_until.store(0, Ordering::SeqCst);
        self.runtime.breaker_until.store(0, Ordering::SeqCst);
        self.runtime.fails.store(0, Ordering::SeqCst);
        self.runtime.soft_streak.store(0, Ordering::SeqCst);
        if let Ok(mut map) = self.runtime.model_cooldowns.write() {
            map.clear();
        }
        if let Ok(mut streaks) = self.runtime.model_streaks.write() {
            streaks.clear();
        }
    }

    /// 任一次成功都要把该账号的惩罚状态清空。
    pub fn on_success(&self, model: &str) {
        self.runtime.cooldown_until.store(0, Ordering::SeqCst);
        self.runtime.breaker_until.store(0, Ordering::SeqCst);
        self.runtime.fails.store(0, Ordering::SeqCst);
        self.runtime.soft_streak.store(0, Ordering::SeqCst);
        if let Ok(mut map) = self.runtime.model_cooldowns.write() {
            map.remove(model);
        }
        if let Ok(mut streaks) = self.runtime.model_streaks.write() {
            streaks.remove(model);
        }
    }

    /// 记住"某个模型被限流到什么时候"（同账号其他模型不受影响）。
    fn park_model(&self, model: &str, until: u64) {
        if let Ok(mut map) = self.runtime.model_cooldowns.write() {
            let entry = map.entry(model.to_string()).or_insert(0);
            // 只延长不缩短：并发下短冷却不能把长冷却覆盖掉。
            if until > *entry {
                *entry = until;
            }
        }
    }

    /// 模型级软限流计数 +1，返回当前值（决定退避倍数）。
    fn bump_model_streak(&self, model: &str) -> u32 {
        let mut streaks = match self.runtime.model_streaks.write() {
            Ok(streaks) => streaks,
            Err(_) => return 1,
        };
        let streak = streaks.entry(model.to_string()).or_insert(0);
        *streak = streak.saturating_add(1);
        *streak
    }

    /// 该账号当前**为什么不可用**（面板状态旁 `?` 的 tooltip 内容）。
    ///
    /// `model` 用于检查**模型级**冷却：上游限流常常只针对单个模型，
    /// 不传就会漏掉最常见的那种"用不了"。
    /// 可用时返回空 —— 不给用户看无意义的信息。
    /// 账号整体不可用的原因。
    ///
    /// **不含模型级限流** —— 那是"某个模型在等"，号本身可用，
    /// 由 `model_cooldowns_snapshot()` 单独表达。
    ///
    /// `model` 保留在签名里是为了兼容既有调用点；当前实现不再按模型过滤。
    pub fn blocked_reasons(&self, _model: &str, now: u64) -> Vec<String> {
        let mut reasons = Vec::new();
        if !self.enabled {
            reasons.push("已停用：不会参与调度".to_string());
        }
        if self.requires_credential && !self.credentials().is_present() {
            reasons.push("没有凭据：该渠道需要先完成登录".to_string());
        }
        if let Some(until) = self.runtime_balance_after(now) {
            reasons.push(format!(
                "余额/额度不足被暂停，{} 后自动恢复",
                humanize(until - now)
            ));
        }
        if let Some(until) = self.runtime_breaker_after(now) {
            reasons.push(format!(
                "连续失败触发熔断，{} 后自动恢复（成功一次会立刻解除）",
                humanize(until - now)
            ));
        }
        if let Some(until) = self.runtime_cooldown_after(now) {
            reasons.push(format!(
                "被上游拒绝或限流，冷却 {} 后自动恢复",
                humanize(until - now)
            ));
        }
        /*
          模型级限流**不再塞进这里**。

          这个列表的语义是"账号整体不可用"，后端据此把 status 算成 `cooldown`。
          模型级限流只是"某个模型在等"，号本身照常服务其他模型 ——
          混进来会让整号显示成冷却中，用户以为号废了去删号重建。
          模型级走 `model_cooldowns_snapshot()`，由界面单独一列展示。
        */
        reasons
    }

    fn runtime_cooldown_after(&self, now: u64) -> Option<u64> {
        let value = self.runtime.cooldown_until.load(Ordering::SeqCst);
        (value > now).then_some(value)
    }

    fn runtime_breaker_after(&self, now: u64) -> Option<u64> {
        let value = self.runtime.breaker_until.load(Ordering::SeqCst);
        (value > now).then_some(value)
    }

    fn runtime_balance_after(&self, now: u64) -> Option<u64> {
        let value = self.runtime.balance_until.load(Ordering::SeqCst);
        (value > now).then_some(value)
    }

    pub fn cooldown_until_ms(&self) -> u64 {
        self.runtime
            .cooldown_until
            .load(Ordering::SeqCst)
            .max(self.runtime.balance_until.load(Ordering::SeqCst))
            .max(self.runtime.breaker_until.load(Ordering::SeqCst))
    }

    /// 模型级冷却快照：(上游模型名, 恢复时刻 ms)。
    ///
    /// 为什么要暴露：上游按**模型**限额是常态（WorkBuddy 会返回 6004）。
    /// 这时账号本身还能跑别的模型，但管理面若只看到"账号不可用"，
    /// 用户会误以为号废了，去删号重建 —— 那才真的丢了配置。
    ///
    /// 只返回**尚未恢复**的项：已经过期的对读者没有意义，
    /// 让界面自己再过滤一遍只会两边口径不一致。
    pub fn model_cooldowns_snapshot(&self, now: u64) -> Vec<(String, u64)> {
        let mut list: Vec<(String, u64)> = self
            .model_cooldowns()
            .iter()
            .filter(|(_, until)| **until > now)
            .map(|(model, until)| (model.clone(), *until))
            .collect();
        // 稳定顺序：HashMap 迭代顺序随机，界面每次刷新都换序会闪。
        list.sort_by(|left, right| left.0.cmp(&right.0));
        list
    }

    /// 清掉全部冷却（手动重置用）。
    ///
    /// 为什么需要手动重置：上游恢复得比退避策略早时，
    /// 自动等待会让号白闲着。管理员看到"其实已经能用"就该能立刻放行。
    /// 一次性清三类 + 模型级，返回是否真的清掉了东西（供接口回报）。
    pub fn reset_cooldowns(&self) -> bool {
        let now = crate::gateway::timeutil::now_ms();
        let had_account = self.cooldown_until_ms() > now;

        self.runtime.cooldown_until.store(0, Ordering::SeqCst);
        self.runtime.balance_until.store(0, Ordering::SeqCst);
        self.runtime.breaker_until.store(0, Ordering::SeqCst);

        let had_models = {
            let mut limits = self
                .runtime
                .model_cooldowns
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let had = !limits.is_empty();
            limits.clear();
            had
        };
        // 退避计数也要清：只清时刻、留着连击数，下次限流会立刻跳到很长的退避。
        if let Ok(mut streaks) = self.runtime.model_streaks.write() {
            streaks.clear();
        }

        had_account || had_models
    }

    pub fn inflight(&self) -> u32 {
        self.runtime.inflight.load(Ordering::SeqCst)
    }

    /// 该账号在飞是否已达上限（调度器据此换号，而不是排队等它）。
    pub fn inflight_full(&self) -> bool {
        self.inflight() >= self.max_inflight()
    }

    fn model_cooldowns(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, u64>> {
        self.runtime
            .model_cooldowns
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn raise_cooldown(&self, slot: &AtomicU64, now: u64, secs: u64) {
        let until = now.saturating_add(secs.saturating_mul(1000));
        if until > slot.load(Ordering::SeqCst) {
            slot.store(until, Ordering::SeqCst);
        }
    }
}

/// 把毫秒差说成人话：38s / 6m / 2h。
pub fn humanize(delta_ms: u64) -> String {
    let seconds = delta_ms / 1000;
    if seconds < 60 {
        return format!("{seconds} 秒");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} 分钟");
    }
    format!("{} 小时", minutes / 60)
}

/// 指数退避：`base · 2^exponent`，封顶 `cap`。
fn backoff_secs(base: u64, exponent: u32, cap: u64) -> u64 {
    let exponent = exponent.min(16);
    base.saturating_mul(1u64 << exponent).min(cap)
}

/// 在飞槽位守卫：请求结束（含客户端断连导致 future 被 drop）时自动归还。
pub struct InflightGuard {
    account: Arc<PoolAccount>,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.account.runtime.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 单个模型的限流窗口（面板"限流等待"列 / 状态 `?` 用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelLimit {
    pub model: String,
    pub until_ms: u64,
}

/// 供测试与面板使用的账号快照视图。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountView {
    pub channel_id: String,
    pub uid: String,
    pub nickname: String,
    pub enabled: bool,
    pub ready: bool,
    pub has_credential: bool,
    pub credential_expires_at_ms: Option<u64>,
    pub cooldown_until_ms: u64,
    /// 不可用原因（冷却 / 熔断 / 额度停放 / 没凭据 / 停用），供面板 tooltip。
    pub blocked_reasons: Vec<String>,
    /// 模型级限流（key = 上游模型名）。账号本身还是可用的，只是这几个模型要等，
    /// 所以单独给面板一份，别和"账号不可用"混在一起。
    pub model_limits: Vec<ModelLimit>,
    /// 当前进行中的请求数。
    pub inflight: u32,
    /// 该账号的并发上限（面板显示"进行中 / 上限"）。
    pub max_inflight: u32,
    pub priority: u32,
    pub weight: u32,
    /// 账号级覆盖值（`None` = 跟随渠道）。编辑弹窗据此决定输入框留空还是填数。
    pub priority_override: Option<u32>,
    pub weight_override: Option<u32>,
    pub proxy_slot: Option<String>,
    /// 这个账号只服务哪些对外模型名（空 = 不限）。面板"编辑账号"回填用。
    pub model_allow: Vec<String>,
    /// 账号级模型改名（对外名 -> 上游名）。
    pub model_rename: std::collections::BTreeMap<String, String>,
    /// 额度快照（渠道自行决定口径；没有额度概念的渠道是 None）。
    pub quota: Option<QuotaSnapshot>,
}

impl PoolAccount {
    pub fn view(&self, now: u64) -> AccountView {
        let credentials = self.credentials();
        AccountView {
            channel_id: self.key.channel_id.clone(),
            uid: self.key.uid.clone(),
            nickname: self.nickname.clone(),
            enabled: self.enabled,
            ready: self.is_ready("", now),
            // 同 `is_present()`：CodeArts 的 AK/SK 没有 token，面板也该显示"有凭据"。
            has_credential: credentials.is_present(),
            credential_expires_at_ms: credentials.expires_at_ms,
            cooldown_until_ms: self.cooldown_until_ms(),
            blocked_reasons: self.blocked_reasons("", now),
            model_limits: {
                let mut limits: Vec<ModelLimit> = self
                    .model_cooldowns()
                    .iter()
                    .filter(|(_, until)| **until > now)
                    .map(|(model, until)| ModelLimit {
                        model: model.clone(),
                        until_ms: *until,
                    })
                    .collect();
                limits.sort_by(|a, b| a.until_ms.cmp(&b.until_ms));
                limits
            },
            inflight: self.inflight(),
            max_inflight: self.max_inflight(),
            priority: self.priority,
            weight: self.weight,
            priority_override: self.priority_override,
            weight_override: self.weight_override,
            proxy_slot: self.proxy_slot.clone(),
            model_allow: self.model_allow.clone(),
            model_rename: self.model_rename.clone(),
            quota: self.quota(),
        }
    }
}

#[cfg(test)]
mod tests;
