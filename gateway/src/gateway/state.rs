//! 网关共享状态：配置、渠道注册表、账号池、并发槽、服务状态与统计。
//!
//! 相对参考项目的变化：渠道与账号从 PG 加载（不再读 `config.json`），
//! 存储层是 `Storage`（PG），没有 Tauri 运行时。

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::gateway::{
    channels::{self, AccountSnapshot, ChannelAdapter},
    config::{Config, RouteOverride},
    core::{
        affinity::AffinityStore,
        matrix::{ChannelCatalog, Matrix},
        pool::{AccountPool, PoolAccount},
    },
    errors::GatewayError,
    storage::{AccountRecord, Storage},
    timeutil::now_ms,
};

/// 进程内计数，重启清零；面板读它做"今天怎么样"的粗看。
///
/// **两个计数器统计的是互斥的两个阶段**，不是"总数里有多少失败"：
/// - `requests`：拿到了上游会话（进入转发阶段）的请求数；
/// - `failures`：选不出号、没能进入转发阶段的请求数。
///
/// 所以总请求数是 `requests + failures`。用 `failures / requests` 算错误率
/// 会得到大于 100% 的值（例如 3 成功 9 失败 → 300%），那种数字没人会当真。
#[derive(Debug, Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub failures: AtomicU64,
    pub retries: AtomicU64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    /// 实际监听地址（端口被占用时可能与配置不同）。
    pub addr: Option<String>,
    pub error: Option<String>,
}

pub type SharedGateway = Arc<GatewayState>;

pub struct GatewayState {
    pub started_at_ms: u64,
    pub client: reqwest::Client,
    /// 计价用的「模型 → 单价」表（由管理面通过 `/internal/prices` 推来）。
    ///
    /// 为什么不由网关自己查库：它的存储层只读需要的列，
    /// 加一张表要连带改连接与缓存策略；而这是一份很小的只读映射，
    /// 缓存在内存里更简单，也不影响现有加载流程。
    price_book: RwLock<Arc<crate::gateway::core::billing::PriceBook>>,
    /// 「分组 → 平台」表（重载渠道时一起刷新）。
    ///
    /// 空分组也要能查适配器：适配器与账号无关，账号只是凭据。
    channel_platforms: RwLock<HashMap<String, String>>,
    /// 「渠道 → 倍率」表（重载渠道时一起刷新）。
    ///
    /// 单独存而不是塞进 `PoolAccount`：倍率是**渠道**属性，
    /// 同一渠道下几十个账号共用一份，复制到每个账号上会浪费内存且容易不一致。
    channel_ratios: RwLock<HashMap<String, f64>>,
    /// 按代理缓存的 HTTP 客户端。
    ///
    /// 为什么必须缓存：`reqwest::Client` 自带连接池，每次请求新建一个
    /// 会丢掉 keep-alive（每个请求都重新握手 TLS，延迟翻倍）。
    /// key 是代理的 `protocol://host:port`，代理配置变更时整体重建。
    proxy_clients: RwLock<HashMap<String, reqwest::Client>>,
    pub pool: AccountPool,
    /// 账号 / 凭据 / 用量 / 亲和的落库层。
    pub storage: Arc<Storage>,
    /// 全局并发闸门（每账号并发在 `PoolAccount::max_inflight`）。
    pub slots: Arc<Semaphore>,
    pub stats: Stats,
    config: RwLock<Config>,
    adapters: RwLock<HashMap<String, Arc<dyn ChannelAdapter>>>,
    /// 会话亲和：保上游前缀缓存命中。
    pub affinity: AffinityStore,
    /// 模型 × 渠道矩阵：懒加载 + TTL，见 `ensure_matrix`。
    matrix: RwLock<Option<Arc<Matrix>>>,
    matrix_lock: tokio::sync::Mutex<()>,
    matrix_built_at: AtomicU64,
    /// 最近的调度决策（面板的"调度"视图与排障用）。
    decisions: RwLock<VecDeque<serde_json::Value>>,
    /// 任务执行进度：key = `channel/uid`。长任务在后台跑，前端轮询这里看进展。
    task_runs: RwLock<HashMap<String, serde_json::Value>>,
    /// 告警发送器（掉号 / 额度不足 / 错误率突增）。
    pub notifier: crate::gateway::notify::Notifier,
    /// 取消标志：key = `channel/uid`（SPEC 11.7）。
    ///
    /// 用 `Arc<AtomicBool>` 而不是直接删 `task_runs` 里的条目：执行线程持有的是
    /// 一个跨 await 的长任务，删条目会让它后面的 `push_task_result` 无处可写，
    /// 也没法区分"取消了"和"从没开始"。标志位让执行线程在**每项之间**主动检查，
    /// 正在飞的上游请求不打断（那是上游的钱，打断也不退）。
    task_cancels: RwLock<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    status: RwLock<ServerStatus>,
    warnings: RwLock<Vec<String>>,
}

impl GatewayState {
    /// 从 PG 加载渠道与账号，建立网关状态。
    pub async fn initialize(
        config: Config,
        storage: Arc<Storage>,
    ) -> Result<SharedGateway, GatewayError> {
        let mut warnings = Vec::new();

        let channels = storage.list_channels().await?;
        let records = storage.list_accounts().await?;
        let adapters = channels::build_registry_from(&channels, &mut warnings);
        let accounts = build_accounts(&config, &adapters, &records, &mut warnings);
        let pool = AccountPool::new(accounts, config.cooldowns.clone());

        let affinity = AffinityStore::new(&config.affinity);
        // 亲和绑定落盘后再恢复：进程重启不该让所有会话换账号（那等于把缓存全丢掉）。
        match storage.load_affinity().await {
            Ok(rows) if !rows.is_empty() => {
                let count = rows.len();
                affinity.restore(rows);
                eprintln!("[gateway] 已恢复 {count} 条会话亲和绑定");
            }
            Ok(_) => {}
            Err(error) => warnings.push(format!("亲和绑定恢复失败：{}", error.message)),
        }

        let slots = Arc::new(Semaphore::new(config.concurrency.global.max(1) as usize));
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            // 只设连接超时：流式响应是长连接，任何总超时都会把长回答掐断。
            .pool_max_idle_per_host(8)
            .user_agent("relayai-gateway/0.1")
            // 禁用系统/环境代理：上游在国内直连本来就能到，
            // 而 reqwest 默认会读系统代理，梯子一关连接立刻全断。
            .no_proxy()
            .build()
            .map_err(|error| GatewayError::internal(format!("HTTP 客户端构建失败：{error}")))?;

        Ok(Arc::new(Self {
            started_at_ms: now_ms(),
            client,
            price_book: RwLock::new(Arc::new(
                crate::gateway::core::billing::PriceBook::default(),
            )),
            channel_ratios: RwLock::new(HashMap::new()),
            channel_platforms: RwLock::new(HashMap::new()),
            proxy_clients: RwLock::new(HashMap::new()),
            pool,
            storage,
            slots,
            stats: Stats::default(),
            config: RwLock::new(config),
            adapters: RwLock::new(adapters),
            affinity,
            matrix: RwLock::new(None),
            matrix_lock: tokio::sync::Mutex::new(()),
            matrix_built_at: AtomicU64::new(0),
            decisions: RwLock::new(VecDeque::with_capacity(DECISION_HISTORY)),
            task_runs: RwLock::new(HashMap::new()),
            task_cancels: RwLock::new(HashMap::new()),
            notifier: crate::gateway::notify::Notifier::new(),
            status: RwLock::new(ServerStatus::default()),
            warnings: RwLock::new(warnings),
        }))
    }

    pub fn config(&self) -> Config {
        self.config
            .read()
            .map(|config| config.clone())
            .unwrap_or_default()
    }

    /// 按**平台**取适配器（注册表现在以平台为键）。
    ///
    /// 名字保留为 `channel` 是为了少改几十处调用点，但语义已经变了：
    /// 传进来的应该是 `account.platform`，不是 `account.key.channel_id`。
    /// 传渠道 id 会拿不到适配器（渠道 id 与平台名不再重合），
    /// 于是调用方会走"渠道不存在"的分支 —— 那种失败是显式的，不会静默打错上游。
    pub fn channel(&self, platform: &str) -> Option<Arc<dyn ChannelAdapter>> {
        self.adapters.read().ok()?.get(platform).cloned()
    }

    /// 取该账号应当使用的 HTTP 客户端（按代理区分）。
    ///
    /// 三种情况：
    ///   1. 没绑代理 → 共享的直连客户端；
    ///   2. 绑了代理且能构建 → 该代理专属的客户端（按 `protocol://host:port` 缓存）；
    ///   3. 绑了代理但构建失败（地址写错/协议不支持）：
    ///      · `fallback = "direct"` → 退回直连（可用性优先）；
    ///      · 否则 → 返回错误（**不静默直连**：那会泄露真实 IP，
    ///        而用户绑代理的目的大概率就是为了隐藏它）。
    pub fn client_for(
        &self,
        account: &crate::gateway::core::pool::PoolAccount,
    ) -> Result<reqwest::Client, GatewayError> {
        let Some(proxy) = account.proxy.as_ref() else {
            return Ok(self.client.clone());
        };

        let key = format!("{}://{}:{}", proxy.protocol, proxy.host, proxy.port);

        // 命中缓存直接返回（连接池复用）。
        if let Ok(cache) = self.proxy_clients.read() {
            if let Some(client) = cache.get(&key) {
                return Ok(client.clone());
            }
        }

        match build_proxy_client(proxy) {
            Ok(client) => {
                if let Ok(mut cache) = self.proxy_clients.write() {
                    cache.insert(key, client.clone());
                }
                Ok(client)
            }
            Err(error) => {
                if proxy.fallback == "direct" {
                    eprintln!(
                        "[gateway] 代理 {} 不可用（{error}），按 fallback=direct 走直连",
                        proxy.name
                    );
                    Ok(self.client.clone())
                } else {
                    Err(GatewayError::internal(format!(
                        "代理 {} 不可用：{error}",
                        proxy.name
                    )))
                }
            }
        }
    }

    /// 代理配置变更后清空缓存（账号重载时调用）。
    pub fn clear_proxy_clients(&self) {
        if let Ok(mut cache) = self.proxy_clients.write() {
            cache.clear();
        }
    }

    /// 替换价格表（管理面推送时调用）。
    pub fn set_price_book(&self, book: crate::gateway::core::billing::PriceBook) {
        if let Ok(mut current) = self.price_book.write() {
            *current = Arc::new(book);
        }
    }

    /// 取当前价格表（读锁后克隆 Arc，避免持有锁做计算）。
    pub fn price_book(&self) -> Arc<crate::gateway::core::billing::PriceBook> {
        self.price_book
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| Arc::new(crate::gateway::core::billing::PriceBook::default()))
    }

    /// 设置渠道倍率表（重载渠道时调用）。
    pub fn set_channel_ratios(&self, ratios: HashMap<String, f64>) {
        if let Ok(mut current) = self.channel_ratios.write() {
            *current = ratios;
        }
    }

    /// 取某渠道的倍率；没配过时按 1.0（不涨价也不打折）。
    pub fn channel_ratio(&self, channel_id: &str) -> f64 {
        self.channel_ratios
            .read()
            .ok()
            .and_then(|guard| guard.get(channel_id).copied())
            .filter(|value| *value > 0.0)
            .unwrap_or(1.0)
    }

    /// 按账号取适配器：读该账号的 platform，再查注册表。
    ///
    /// 这是"平台在账号上"之后**唯一正确**的取法。直接用渠道 id 取会失败，
    /// 因为一个渠道下面可能混着不同平台的号。
    pub fn adapter_for(&self, account: &crate::gateway::core::pool::PoolAccount) -> Option<Arc<dyn ChannelAdapter>> {
        let platform = if account.platform.is_empty() {
            // 老数据/异常数据没带平台：退回 fake 之外什么都不给，
            // 让上层显式报"账号没有平台"，而不是猜一个去打上游。
            return None;
        } else {
            account.platform.as_str()
        };
        self.channel(platform)
    }

    /// 按**渠道**取适配器：用该渠道名下第一个账号的平台。
    ///
    /// 管理面有些接口天生是"按渠道"的（渠道模型目录、渠道探活、渠道能力），
    /// 而适配器现在挂在平台上。这里做一次桥接：取该渠道第一个账号的平台。
    ///
    /// 局限必须说清楚：**一个渠道下混多平台时，这个接口只反映第一个账号的平台**。
    /// 所以这些接口的返回都带 `platform` 字段，让调用方知道它代表的是谁；
    /// 需要账号级精确结果的地方（探活、任务）走 `/internal/accounts/...`。
    pub fn adapter_for_channel(&self, channel_id: &str) -> Option<Arc<dyn ChannelAdapter>> {
        let accounts = self.pool.by_channel(channel_id);
        if let Some(first) = accounts.first() {
            return self.adapter_for(first);
        }

        /*
            分组下一个账号都没有时的兜底。

            典型场景：刚建好分组就去点「渠道任务」，或者账号全被停用/删除。
            旧实现直接返回 None，接口报「渠道不存在」—— 但分组明明存在，
            于是界面显示"加载失败"，把人引向"网关是不是挂了"这个错误方向。

            这里退回到「该分组配置的 adapter」：分组表里记着它属于哪个平台，
            适配器本身与账号无关（账号只是凭据）。这样空分组也能看到任务与能力，
            只是没有号可跑。
        */
        let platform = self
            .channel_platforms
            .read()
            .ok()
            .and_then(|map| map.get(channel_id).cloned())?;

        self.channel(&platform)
    }

    /**
     * 同步版兜底：查不到内存缓存时直接问数据库。

     * 为什么需要它：`channel_platforms` 只在启动/重载渠道时刷新，
     * 运行中**新建的分组**（比如刚加的国际版）不在里面 —— 于是空分组依旧报
     * 「渠道不存在」，用户看到"加载失败"，以为网关挂了。

     * 这里读 `channels.adapter` 补上：一次索引查询，只在缓存未命中时才发生。
     */
    pub async fn adapter_for_channel_async(
        &self,
        channel_id: &str,
    ) -> Option<Arc<dyn ChannelAdapter>> {
        if let Some(adapter) = self.adapter_for_channel(channel_id) {
            return Some(adapter);
        }

        let platform = self.storage.channel_adapter(channel_id).await.ok().flatten()?;
        // 顺手补进缓存，后续请求不必再查库。
        if let Ok(mut slot) = self.channel_platforms.write() {
            slot.insert(channel_id.to_string(), platform.clone());
        }
        self.channel(&platform)
    }

    /// 该渠道名下出现过的平台集合（去重、排序）。
    ///
    /// 用于界面显示"这个渠道里有哪些类型的号"，也用于判断"渠道是否混平台"。
    pub fn platforms_of_channel(&self, channel_id: &str) -> Vec<String> {
        let mut platforms: Vec<String> = self
            .pool
            .by_channel(channel_id)
            .iter()
            .map(|account| account.platform.clone())
            .filter(|platform| !platform.is_empty())
            .collect();
        platforms.sort();
        platforms.dedup();
        platforms
    }

    /// 平台列表按名字排序：HashMap 的迭代顺序不确定，而 `/v1/models` 要对用户稳定。
    pub fn channels(&self) -> Vec<Arc<dyn ChannelAdapter>> {
        let mut list: Vec<Arc<dyn ChannelAdapter>> = self
            .adapters
            .read()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by(|a, b| a.id().cmp(b.id()));
        list
    }

    pub fn first_account_for(&self, channel_id: &str) -> Option<AccountSnapshot> {
        self.pool
            .by_channel(channel_id)
            .first()
            .map(|account| account.snapshot())
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.read().map(|list| list.clone()).unwrap_or_default()
    }

    pub fn set_server_status(&self, status: ServerStatus) {
        if let Ok(mut slot) = self.status.write() {
            *slot = status;
        }
    }

    pub fn server_status(&self) -> ServerStatus {
        self.status
            .read()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn uptime_ms(&self) -> u64 {
        now_ms().saturating_sub(self.started_at_ms)
    }

    pub fn global_available(&self) -> usize {
        self.slots.available_permits()
    }

    /// Read the account's own catalog before applying any group publication policy.
    pub async fn models_for_account(
        &self,
        account: &PoolAccount,
    ) -> Result<Vec<channels::UpstreamModel>, GatewayError> {
        let Some(adapter) = self.channel(&account.platform) else {
            return Ok(Vec::new());
        };
        let mut models = adapter
            .list_models(&self.client, &account.snapshot())
            .await?
            .unwrap_or_default();
        models.retain(|model| {
            account.model_allow.is_empty() || account.model_allow.contains(&model.id)
        });
        Ok(models)
    }

    /// 取当前矩阵；过期或不存在时按需重建（TTL + 双检锁）。
    pub async fn ensure_matrix(&self, force: bool) -> Arc<Matrix> {
        let ttl_ms = self.config().catalog_ttl_seconds.saturating_mul(1000);
        if !force {
            if let Some(matrix) = self.matrix.read().ok().and_then(|slot| slot.clone()) {
                if now_ms().saturating_sub(self.matrix_built_at.load(Ordering::Relaxed)) < ttl_ms {
                    return matrix;
                }
            }
        }
        let _guard = self.matrix_lock.lock().await;
        if !force {
            if let Some(matrix) = self.matrix.read().ok().and_then(|slot| slot.clone()) {
                if now_ms().saturating_sub(self.matrix_built_at.load(Ordering::Relaxed)) < ttl_ms {
                    return matrix;
                }
            }
        }

        let config = self.config();
        let mut catalogs = Vec::new();
        let mut overrides = Vec::new();
        let groups = self.storage.list_channels().await.unwrap_or_default();
        for channel in groups.iter().filter(|channel| channel.enabled) {
            // Aggregate real assigned accounts, including each custom account's own catalog.
            let mut models = Vec::new();
            let accounts = self.pool.by_channel(&channel.id);
            for account in accounts.iter().filter(|account| account.enabled) {
                if let Ok(entries) = self.models_for_account(account).await {
                    for model in entries {
                        if !channel.model_allow.is_empty() && !channel.model_allow.contains(&model.id) {
                            overrides.push(RouteOverride {
                                model: model.id.clone(),
                                channel_id: channel.id.clone(),
                                enabled: Some(false),
                                ..Default::default()
                            });
                        }
                        if !models.iter().any(|existing: &channels::UpstreamModel| existing.id == model.id) {
                            models.push(model);
                        }
                    }
                }
            }
            if models.is_empty() {
                continue;
            }
            catalogs.push(ChannelCatalog {
                channel_id: channel.id.clone(),
                priority: channel.priority,
                weight: channel.weight,
                models,
                live: true,
            });
        }

        let matrix = Arc::new(Matrix::build(&catalogs, &config.aliases, &overrides));
        if let Ok(mut slot) = self.matrix.write() {
            *slot = Some(Arc::clone(&matrix));
        }
        self.matrix_built_at.store(now_ms(), Ordering::Relaxed);
        matrix
    }

    pub fn matrix_snapshot(&self) -> Option<Arc<Matrix>> {
        self.matrix.read().ok().and_then(|slot| slot.clone())
    }

    /// 记一条调度决策（内存环形缓冲，供面板"调度"视图）。
    pub fn push_decision(&self, entry: serde_json::Value) {
        if let Ok(mut decisions) = self.decisions.write() {
            if decisions.len() >= DECISION_HISTORY {
                decisions.pop_front();
            }
            decisions.push_back(entry);
        }
    }

    /// 最近的决策，最新在前。
    pub fn decisions(&self) -> Vec<serde_json::Value> {
        self.decisions
            .read()
            .map(|decisions| decisions.iter().rev().cloned().collect())
            .unwrap_or_default()
    }

    /// 账号表变化后重建账号池（登录成功、启停、删除都走它）。
    pub async fn reload_accounts(&self) -> Result<usize, GatewayError> {
        let records = self.storage.list_accounts().await?;
        let config = self.config();
        let adapters = self
            .adapters
            .read()
            .map(|adapters| adapters.clone())
            .unwrap_or_default();
        let mut warnings = Vec::new();
        let accounts = build_accounts(&config, &adapters, &records, &mut warnings);
        let count = accounts.len();
        self.pool.replace(accounts);
        if let Ok(mut slot) = self.matrix.write() {
            *slot = None;
        }
        self.matrix_built_at.store(0, Ordering::Relaxed);
        if !warnings.is_empty() {
            if let Ok(mut slot) = self.warnings.write() {
                *slot = warnings;
            }
        }
        Ok(count)
    }

    /// 渠道表变化后重建适配器与账号池（管理面改渠道时调用）。
    pub async fn reload_channels(&self) -> Result<(), GatewayError> {
        let channels = self.storage.list_channels().await?;
        let mut warnings = Vec::new();
        let adapters = channels::build_registry_from(&channels, &mut warnings);

        // 顺带刷新渠道倍率：它和渠道同源，分开加载会让"改了倍率但没生效"
        // 变成一类只在重启后自愈的怪问题。
        let ratios: HashMap<String, f64> = channels
            .iter()
            .map(|channel| (channel.id.clone(), channel.channel_ratio))
            .collect();
        self.set_channel_ratios(ratios);

        // 分组 → 平台：空分组查适配器时用（见 `adapter_for_channel`）。
        let platforms: HashMap<String, String> = channels
            .iter()
            .map(|channel| (channel.id.clone(), channel.adapter.clone()))
            .collect();
        if let Ok(mut slot) = self.channel_platforms.write() {
            *slot = platforms;
        }

        if let Ok(mut slot) = self.adapters.write() {
            *slot = adapters;
        }
        if let Ok(mut slot) = self.warnings.write() {
            *slot = warnings;
        }

        // 矩阵依赖渠道目录，渠道变了必须重建。
        if let Ok(mut slot) = self.matrix.write() {
            *slot = None;
        }
        self.matrix_built_at.store(0, Ordering::Relaxed);

        self.reload_accounts().await?;
        Ok(())
    }

    /// 按 key 找账号（面板操作与登录落库后使用）。
    pub fn account(&self, channel_id: &str, uid: &str) -> Option<Arc<PoolAccount>> {
        self.pool.get(&crate::gateway::core::pool::AccountKey::new(channel_id, uid))
    }

    /// 按明文查 API Key 的权限记录（SPEC 12.1）。
    ///
    /// 库里只存 SHA-256，所以这里先哈希再查；明文不进内存索引、不进日志。
    pub async fn lookup_api_key(
        &self,
        plaintext: &str,
    ) -> Result<Option<crate::gateway::storage::ApiKeyRecord>, GatewayError> {
        self.storage.lookup_api_key(plaintext).await
    }

    /// 库里是否有任何 Key。
    ///
    /// 用于区分"没配 Key（本地免鉴权）"与"配了 Key 但这次没带"——
    /// 前者放行，后者 401。混为一谈会让免鉴权模式在配了 Key 之后仍然敞开。
    pub async fn has_api_keys(&self) -> bool {
        self.storage.count_api_keys().await.unwrap_or(0) > 0
    }

    /// 开始一次任务执行；已在执行中返回 false（调用方据此跳过该账号）。
    pub fn begin_task_run(&self, channel_id: &str, uid: &str) -> bool {
        let Ok(mut runs) = self.task_runs.write() else {
            return false;
        };
        let key = format!("{channel_id}/{uid}");
        if runs
            .get(&key)
            .map(|entry| entry["running"].as_bool().unwrap_or(false))
            .unwrap_or(false)
        {
            return false;
        }

        // 新一轮执行必须清掉上一次遗留的取消标志，否则新任务立刻就被判定取消。
        if let Ok(mut cancels) = self.task_cancels.write() {
            cancels.insert(
                key.clone(),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
            );
        }

        runs.insert(
            key,
            serde_json::json!({
                "running": true,
                "started_at": now_ms(),
                "finished_at": serde_json::Value::Null,
                "cancelled": false,
                "results": [],
                "done": 0,
                "earned": 0,
            }),
        );
        true
    }

    /// 请求取消某个账号正在执行的任务（SPEC 11.7）。
    ///
    /// 只置标志位，不打断正在飞的上游请求：那个请求的钱已经花了，打断也不退。
    /// 执行线程在**每项之间**检查这个标志，据此停止后续项。
    ///
    /// 返回 false 表示该账号当前没有执行中的任务（调用方可以据此提示"已经结束"）。
    pub fn cancel_task_run(&self, channel_id: &str, uid: &str) -> bool {
        let key = format!("{channel_id}/{uid}");
        {
            let Ok(runs) = self.task_runs.read() else {
                return false;
            };
            let running = runs
                .get(&key)
                .map(|entry| entry["running"].as_bool().unwrap_or(false))
                .unwrap_or(false);
            if !running {
                return false;
            }
        }
        if let Ok(cancels) = self.task_cancels.read() {
            if let Some(flag) = cancels.get(&key) {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    /// 该账号的任务是否已被要求取消（执行线程每项之间调用）。
    pub fn task_cancelled(&self, channel_id: &str, uid: &str) -> bool {
        self.task_cancels
            .read()
            .ok()
            .and_then(|cancels| cancels.get(&format!("{channel_id}/{uid}")).cloned())
            .map(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// 推一条任务结果（后台执行线程调用）。
    pub fn push_task_result(
        &self,
        channel_id: &str,
        uid: &str,
        result: crate::gateway::channels::TaskRunResult,
    ) {
        let Ok(mut runs) = self.task_runs.write() else {
            return;
        };
        let Some(entry) = runs.get_mut(&format!("{channel_id}/{uid}")) else {
            return;
        };
        if let Some(list) = entry["results"].as_array_mut() {
            list.push(serde_json::to_value(result).unwrap_or(serde_json::Value::Null));
        }
    }

    pub fn finish_task_run(&self, channel_id: &str, uid: &str, done: u32, earned: u32) {
        let Ok(mut runs) = self.task_runs.write() else {
            return;
        };
        if let Some(entry) = runs.get_mut(&format!("{channel_id}/{uid}")) {
            entry["running"] = serde_json::json!(false);
            entry["finished_at"] = serde_json::json!(now_ms());
            entry["done"] = serde_json::json!(done);
            entry["earned"] = serde_json::json!(earned);
        }
    }

    pub fn fail_task_run(&self, channel_id: &str, uid: &str, message: &str) {
        let Ok(mut runs) = self.task_runs.write() else {
            return;
        };
        if let Some(entry) = runs.get_mut(&format!("{channel_id}/{uid}")) {
            entry["running"] = serde_json::json!(false);
            entry["finished_at"] = serde_json::json!(now_ms());
            entry["error"] = serde_json::json!(message);
        }
    }

    /// 读任务执行进度（前端轮询）。
    pub fn task_progress(&self, channel_id: &str, uid: &str) -> serde_json::Value {
        self.task_runs
            .read()
            .ok()
            .and_then(|runs| runs.get(&format!("{channel_id}/{uid}")).cloned())
            .unwrap_or_else(|| {
                serde_json::json!({
                    "running": false,
                    "results": [],
                    "done": 0,
                    "earned": 0,
                })
            })
    }
}

/// 内存里保留多少条决策（面板一次看不到更多）。
const DECISION_HISTORY: usize = 200;

/// 按一条代理记录构建专属 HTTP 客户端。
///
/// 与直连客户端只差 `proxy(...)` 一项，其余参数（连接超时、连接池、UA）保持一致 ——
/// 否则"走代理"和"直连"的行为会在别处悄悄不同，排障时很难对齐。
///
/// `no_proxy()` 仍然要显式关掉**系统代理**：我们要用的是配置里这一条，
/// 而不是环境变量里的 `HTTP_PROXY`。两者叠加会出现"套了两层代理"。
fn build_proxy_client(proxy: &crate::gateway::storage::ProxyRecord) -> Result<reqwest::Client, String> {
    let scheme = match proxy.protocol.as_str() {
        "socks5" | "socks5h" => "socks5",
        "http" => "http",
        "https" => "https",
        other => return Err(format!("不支持的代理协议：{other}")),
    };

    // 用户名密码要百分号编码后再拼进 URL：密码里的 `@` / `:` / `/`
    // 手拼会拼出一个语法正确但指向别处的 URL，而错误信息只说"连接失败"。
    let auth = if proxy.username.is_empty() {
        String::new()
    } else {
        let user = urlencode(&proxy.username);
        let pass = urlencode(&proxy.password);
        format!("{user}:{pass}@")
    };

    let url = format!("{scheme}://{auth}{}:{}", proxy.host, proxy.port);
    let parsed = reqwest::Proxy::all(&url).map_err(|error| format!("代理地址无效：{error}"))?;

    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(8)
        .user_agent("relayai-gateway/0.1")
        .no_proxy()
        .proxy(parsed)
        .build()
        .map_err(|error| format!("客户端构建失败：{error}"))
}

/// 最小的百分号编码：只转义会破坏 URL 结构的字符。
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 按落库记录组装账号。
///
/// **适配器按 `record.platform` 选，不是按 `channel_id`。**
/// 平台下沉到账号之后，注册表的键是平台名；用渠道 id 去查会全部落空，
/// 表现为"加载 0 个账号"而配置看起来完全正常 —— 这正是改这一行之前的实际故障。
///
/// 平台未知（写错或适配器未实现）的账号跳过并记警告，不让整个网关起不来。
fn build_accounts(
    config: &Config,
    adapters: &HashMap<String, Arc<dyn ChannelAdapter>>,
    records: &[AccountRecord],
    warnings: &mut Vec<String>,
) -> Vec<Arc<PoolAccount>> {
    let per_account = config.concurrency.per_account.max(1);
    records
        .iter()
        .filter_map(|record| {
            let platform = record.platform.trim();
            let Some(adapter) = adapters.get(platform) else {
                warnings.push(format!(
                    "账号 {}/{} 的平台「{}」没有可用适配器，已跳过",
                    record.channel_id, record.uid, platform
                ));
                return None;
            };
            let mut pooled = PoolAccount::new(record.channel_id.clone(), record.uid.clone());
            pooled.platform = platform.to_string();
            pooled.options = record.options.clone();
            pooled.proxy = record.proxy.clone();
            pooled.nickname = if record.nickname.is_empty() {
                record.uid.clone()
            } else {
                record.nickname.clone()
            };
            pooled.enabled = record.enabled;
            pooled.requires_credential = adapter.requires_credential();
            pooled.static_credential = adapter.static_credential();
            pooled.priority_override = record.priority_override;
            pooled.weight_override = record.weight_override;
            pooled.priority = record.priority_override.unwrap_or(100);
            pooled.weight = record.weight_override.unwrap_or(1);
            pooled.set_max_inflight(record.max_inflight.unwrap_or(per_account));
            pooled.model_allow = record.model_allow.clone();
            pooled.model_rename = record.model_rename.clone();
            pooled.base_url = record.base_url.clone();
            pooled.protocol = record.protocol.clone();
            pooled.platform = record.platform.clone();
            pooled.options = record.options.clone();
            if let Some(credential) = &record.credential {
                pooled.set_credentials(credential.clone());
            }
            Some(Arc::new(pooled))
        })
        .collect()
}
