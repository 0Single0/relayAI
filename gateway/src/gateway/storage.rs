//! 存储层：PostgreSQL（sqlx）。
//!
//! 相对参考项目的变化（SPEC 10.3）：
//!   · SQLite → PostgreSQL，`ts` 用 `timestamptz` 而不是 epoch 毫秒；
//!   · 账号/渠道的唯一事实来源从 `config.json` + SQLite 改为 PG（Go 管理面写入）；
//!   · 凭据密文从 TEXT 改为 BYTEA；
//!   · `model_allow` / `model_rename` 用 PG 的 `text[]` 与 `jsonb`。
//!
//! 用量写入仍是"不阻塞请求路径"：这里用 `tokio::spawn` 异步落库，
//! 失败只记日志（统计少一条，不该让请求失败）。

use std::sync::Arc;

use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::gateway::{
    channels::Credential,
    config::{AccountConfig, ChannelConfig},
    errors::GatewayError,
    protocol::cir::Usage,
    security::envelope,
};

/// 账号落库记录（含解密后的凭据）。
#[derive(Debug, Clone)]
pub struct AccountRecord {
    pub channel_id: String,
    pub uid: String,
    pub nickname: String,
    pub enabled: bool,
    pub priority_override: Option<u32>,
    pub weight_override: Option<u32>,
    pub max_inflight: Option<u32>,
    pub credential: Option<Credential>,
    pub model_allow: Vec<String>,
    pub model_rename: std::collections::BTreeMap<String, String>,
    /// 上游基址（账号级）。空 = 用适配器内置默认。
    ///
    /// 为什么在账号上而不是渠道上：渠道是计价层，一份计价策略下面可以挂
    /// 多个不同上游地址的账号（自建中转 + 官方直连）。地址属于凭据，不属于计价。
    pub base_url: String,
    /// 上游协议（账号级）：`openai-chat` / `openai-responses` / `anthropic`。
    ///
    /// 同一把 Key 可能只支持其中一种，所以它是凭据属性。
    pub protocol: String,
    /// 该账号的平台：`workbuddy` / `trae` / `custom` / `fake`。
    ///
    /// **平台在账号上，不在渠道上。** 渠道是计价分组，一个分组下面可以混
    /// 不同平台的号；"我要加一个 WorkBuddy 号"不需要先建一个 WorkBuddy 渠道。
    pub platform: String,
    /// 适配器私有参数（workbuddy 的 `realm` / `outbound_identity` 等）。
    pub options: serde_json::Value,
    /// 出站代理（账号级）。`None` = 直连。
    ///
    /// 上游按出口 IP 限流：所有账号共用一个 IP 时，几十个号叠在一起
    /// 很容易被判定成异常流量，表现为"偶发 429/403"。
    pub proxy: Option<ProxyRecord>,
}

/// 一条出站代理。
#[derive(Debug, Clone)]
pub struct ProxyRecord {
    pub id: i64,
    pub name: String,
    /// `socks5` / `http` / `https`。
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 已解密的密码。空串 = 无认证。
    pub password: String,
    /// 代理不可用时的策略：`fail`（默认，不泄露真实 IP）/ `direct`。
    pub fallback: String,
}

/// 一条请求的用量记录。
#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub request_id: String,
    pub key_id: Option<i64>,
    pub model: String,
    pub upstream_model: Option<String>,
    pub channel_id: String,
    pub account_uid: String,
    pub protocol: String,
    pub status: String,
    pub is_rejection: bool,
    pub stream: bool,
    pub attempts: usize,
    pub usage: Usage,
    pub estimated: bool,
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
    pub effort: Option<String>,
    pub error_detail: Option<String>,
    /// 上游成本（元）。由倍率计算模块填入；拿不到价格时为 0。
    pub upstream_cost: f64,
    /// 记账额度 = upstream_cost × model_ratio × channel_ratio。
    pub billed_cost: f64,
    /// 计费用的价格与倍率**快照**。
    ///
    /// 为什么存快照而不是事后反查：价格会变（运营调价、拉 OpenRouter）。
    /// 不存的话，用户看到一条历史费用想问"怎么算的"，只能用今天的价去算昨天的账，
    /// 必然对不上 —— 对账时这点是致命的。
    pub price_snapshot: Option<PriceSnapshot>,
    /// 请求/响应内容（只在 `payload.enabled` 打开时填）。
    pub payload: Option<UsagePayload>,
}

/// 一条用量记录的价格快照。
#[derive(Debug, Clone, Copy)]
pub struct PriceSnapshot {
    /// 单价（元 / 百万 token），**未乘任何倍率**。
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    /// 记账额度 = 上游成本 × model_ratio × channel_ratio。
    pub model_ratio: f64,
    pub channel_ratio: f64,
}

/// 留存下来的请求/响应内容（脱敏后的 JSON 字符串）。
#[derive(Debug, Clone)]
pub struct UsagePayload {
    pub request: String,
    pub response: String,
}

/// 存储层。持有连接池与主密钥。
pub struct Storage {
    pool: PgPool,
    master_key: [u8; 32],
}

impl Storage {
    /// 主密钥（只读暴露给内部接口的"加密一段文本"用）。
    ///
    /// 为什么可以暴露：它返回的是 `&[u8; 32]`，只在进程内可见；
    /// 内部接口本身还要 `x-internal-token` 才能调。
    /// 真要防的是"管理面被攻破"，那时攻击者也只能让网关替它加密，
    /// 拿不到密钥本身、也解不开已有密文。
    pub fn master_key(&self) -> [u8; 32] {
        self.master_key
    }

    /// 内容留存是否开启（每次现读 `settings`）。
    ///
    /// 为什么不缓存：这个开关是隐私相关的，用户在界面上关掉之后
    /// **下一条**请求就该停止留存。缓存 60 秒意味着还有 60 秒在偷偷记录。
    /// 代价是每条请求多一次极小的查询，而这只在打开时才发生
    /// （关闭时 `request_json` 先读这个值，读到 false 就直接返回）。
    pub async fn payload_enabled(&self) -> bool {
        const QUERY: &str = "SELECT value FROM settings WHERE key = 'payload.enabled'";
        match sqlx::query_scalar::<_, serde_json::Value>(QUERY)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(Some(value)) => value.as_bool().unwrap_or(false),
            // 读不到（库没起来/键不存在）→ 当作关闭：隐私功能失败要往安全侧倒。
            _ => false,
        }
    }

    /// 打开连接池并校验主密钥。
    pub async fn open(data_source: &str, master_key: [u8; 32]) -> Result<Arc<Self>, GatewayError> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .min_connections(2)
            .connect(data_source)
            .await
            .map_err(|error| GatewayError::internal(format!("连接数据库失败：{error}")))?;

        Ok(Arc::new(Self { pool, master_key }))
    }

    /// 供内部模块（任务、探活）直接查库。
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 加载所有渠道。
    pub async fn list_channels(&self) -> Result<Vec<ChannelConfig>, GatewayError> {
        let rows = sqlx::query_as::<_, ChannelRow>(
            "SELECT id, name, adapter, enabled, priority, weight, options,
                    channel_ratio::float8 AS channel_ratio,
                    COALESCE(model_allow, ARRAY[]::text[]) AS model_allow
             FROM channels ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(rows
            .into_iter()
            .map(|row| ChannelConfig {
                id: row.id,
                adapter: row.adapter,
                display_name: row.name,
                enabled: row.enabled,
                priority: row.priority.max(0) as u32,
                weight: row.weight.max(1) as u32,
                options: row.options,
                channel_ratio: row.channel_ratio,
                model_allow: row.model_allow,
            })
            .collect())
    }

    /// 查一个渠道的 adapter（平台名）。
    ///
    /// 只给"空分组查适配器"这一条兜底路径用：
    /// `channel_platforms` 缓存只在启动/重载时刷新，运行中新建的分组不在里面，
    /// 那时直接查库比报「渠道不存在」准确得多。
    /// 返回 `Ok(None)` 表示库里也没有这个渠道。
    pub async fn channel_adapter(&self, channel_id: &str) -> Result<Option<String>, GatewayError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT adapter FROM channels WHERE id = $1 AND enabled",
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(row.map(|(adapter,)| adapter))
    }

    /// 加载所有账号（含解密后的凭据）。
    ///
    /// 解密失败时**保留账号但不带凭据**：一条坏密文不该让整个号池起不来，
    /// 账号会显示为"无凭据"，用户可以重新登录。
    pub async fn list_accounts(&self) -> Result<Vec<AccountRecord>, GatewayError> {
        let rows = sqlx::query_as::<_, AccountRow>(
            "SELECT a.channel_id, a.uid, a.nickname, a.enabled,
                    a.priority_override, a.weight_override, a.max_inflight,
                    a.credential_cipher, a.model_allow, a.model_rename,
                    a.base_url, a.protocol, a.platform, a.options,
                    p.id AS proxy_id, p.name AS proxy_name, p.protocol AS proxy_protocol,
                    p.host AS proxy_host, p.port AS proxy_port,
                    p.username AS proxy_username, p.password_enc AS proxy_password_enc,
                    p.fallback AS proxy_fallback, p.enabled AS proxy_enabled
             FROM accounts a
             LEFT JOIN proxies p ON p.id = a.proxy_id
             ORDER BY a.channel_id, a.added_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let credential = row.credential_cipher.as_ref().and_then(|cipher| {
                    let text = String::from_utf8_lossy(cipher);
                    match envelope::open(&self.master_key, &text) {
                        Ok(json) => serde_json::from_str::<Credential>(&json).ok(),
                        Err(error) => {
                            eprintln!(
                                "[gateway] 账号 {}/{} 凭据解密失败：{error}",
                                row.channel_id.clone().unwrap_or_default(), row.uid
                            );
                            None
                        }
                    }
                });

                // 代理：只有绑定且**启用**时才生效。
                // 停用的代理当作"没绑"处理，而不是报错 ——
                // 用户停用它通常是想临时绕开一个坏代理，不该因此让账号不可用。
                let proxy = match (row.proxy_id, row.proxy_enabled.unwrap_or(false)) {
                    (Some(id), true) => {
                        let password = row
                            .proxy_password_enc
                            .as_ref()
                            .filter(|cipher| !cipher.is_empty())
                            .and_then(|cipher| match envelope::open(&self.master_key, cipher) {
                                Ok(plain) => Some(plain),
                                Err(error) => {
                                    eprintln!("[gateway] 代理 {id} 密码解密失败：{error}");
                                    None
                                }
                            })
                            .unwrap_or_default();

                        Some(ProxyRecord {
                            id,
                            name: row.proxy_name.unwrap_or_default(),
                            protocol: row.proxy_protocol.unwrap_or_else(|| "socks5".into()),
                            host: row.proxy_host.unwrap_or_default(),
                            port: row.proxy_port.unwrap_or(0).max(0) as u16,
                            username: row.proxy_username.unwrap_or_default(),
                            password,
                            fallback: row.proxy_fallback.unwrap_or_else(|| "fail".into()),
                        })
                    }
                    _ => None,
                };

                AccountRecord {
                    channel_id: row.channel_id.unwrap_or_default(),
                    uid: row.uid,
                    nickname: row.nickname,
                    enabled: row.enabled,
                    priority_override: row.priority_override.map(|v| v.max(0) as u32),
                    weight_override: row.weight_override.map(|v| v.max(1) as u32),
                    max_inflight: row.max_inflight.map(|v| v.max(1) as u32),
                    credential,
                    model_allow: row.model_allow.unwrap_or_default(),
                    model_rename: row
                        .model_rename
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default(),
                    base_url: row.base_url,
                    protocol: row.protocol,
                    platform: row.platform,
                    options: row.options,
                    proxy,
                }
            })
            .collect())
    }

    /// 保存（或更新）账号凭据。
    pub async fn set_credential(
        &self,
        channel_id: &str,
        uid: &str,
        credential: Option<&Credential>,
    ) -> Result<(), GatewayError> {
        self.set_credential_by(channel_id, uid, credential).await
    }

    /// 按 uid 定位写凭据。
    ///
    /// 为什么需要：账号可以先建、后分配分组 —— 未分配的行 `channel_id` 是 NULL，
    /// `(channel_id, uid)` 的 WHERE 永远匹配不到它。
    /// uid 是唯一键（`accounts_uid_key`），用它定位才完整。
    pub async fn set_credential_by(
        &self,
        _channel_id: &str,
        uid: &str,
        credential: Option<&Credential>,
    ) -> Result<(), GatewayError> {
        let cipher = match credential {
            Some(value) => {
                let json = serde_json::to_string(value)
                    .map_err(|error| GatewayError::internal(format!("凭据序列化失败：{error}")))?;
                let sealed = envelope::seal(&self.master_key, &json)
                    .map_err(|error| GatewayError::internal(format!("凭据加密失败：{error}")))?;
                Some(sealed.into_bytes())
            }
            None => None,
        };

        sqlx::query(
            "UPDATE accounts SET credential_cipher = $2 WHERE uid = $1",
        )
        .bind(uid)
        .bind(cipher)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(())
    }

    /// 记一条用量。
    ///
    /// 异步落库：调用方（请求处理路径）不等待。失败只打日志，
    /// 因为"统计少一条"远好过"请求因为统计失败而失败"。
    pub fn record_usage(&self, record: UsageRecord) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            if let Err(error) = insert_usage(&pool, &record).await {
                eprintln!("[gateway] 用量落库失败：{error}");
            }
        });
    }

    /// 保存会话亲和绑定。
    ///
    /// 亲和是**运行态**（SPEC I5），按说不该落库；但进程重启后全部换号等于把
    /// 上游缓存全丢掉，代价太大。所以折中：落一张小表，只存绑定关系，
    /// 不做统计查询，TTL 到期由内存淘汰、库里的行定期清理。
    pub fn save_affinity(
        &self,
        key: &str,
        binding: &crate::gateway::core::affinity::Binding,
    ) -> Result<(), GatewayError> {
        let pool = self.pool.clone();
        let key = key.to_string();
        let binding = binding.clone();

        // 亲和写库在请求热路径上，用 spawn 异步写：失败只影响重启后的缓存命中率。
        tokio::spawn(async move {
            let result = sqlx::query(
                "INSERT INTO affinity_bindings (key, channel_id, uid, upstream_model, expires_at_ms, hits)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (key) DO UPDATE SET
                     channel_id = EXCLUDED.channel_id,
                     uid = EXCLUDED.uid,
                     upstream_model = EXCLUDED.upstream_model,
                     expires_at_ms = EXCLUDED.expires_at_ms,
                     hits = affinity_bindings.hits + 1",
            )
            .bind(&key)
            .bind(&binding.channel_id)
            .bind(&binding.uid)
            .bind(&binding.upstream_model)
            .bind(binding.expires_at_ms as i64)
            .bind(binding.hits as i32)
            .execute(&pool)
            .await;

            if let Err(error) = result {
                eprintln!("[gateway] 亲和绑定落库失败：{error}");
            }
        });

        Ok(())
    }

    /// 删除会话亲和绑定。
    pub fn delete_affinity(&self, key: &str) -> Result<(), GatewayError> {
        let pool = self.pool.clone();
        let key = key.to_string();

        tokio::spawn(async move {
            if let Err(error) = sqlx::query("DELETE FROM affinity_bindings WHERE key = $1")
                .bind(&key)
                .execute(&pool)
                .await
            {
                eprintln!("[gateway] 亲和绑定删除失败：{error}");
            }
        });

        Ok(())
    }

    /// 启动时恢复亲和绑定（未过期的）。
    pub async fn load_affinity(
        &self,
    ) -> Result<Vec<(String, crate::gateway::core::affinity::Binding)>, GatewayError> {
        let now = crate::gateway::timeutil::now_ms() as i64;

        let rows = sqlx::query_as::<_, AffinityRow>(
            "SELECT key, channel_id, uid, upstream_model, expires_at_ms, hits
             FROM affinity_bindings WHERE expires_at_ms > $1",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.key,
                    crate::gateway::core::affinity::Binding {
                        channel_id: row.channel_id,
                        uid: row.uid,
                        upstream_model: row.upstream_model,
                        expires_at_ms: row.expires_at_ms.max(0) as u64,
                        hits: row.hits.max(0) as u32,
                    },
                )
            })
            .collect())
    }

    /// 按明文查 API Key 的权限记录。
    ///
    /// 库里只有 SHA-256：明文在这一层哈希后即丢弃，不进任何缓存或日志。
    pub async fn lookup_api_key(
        &self,
        plaintext: &str,
    ) -> Result<Option<ApiKeyRecord>, GatewayError> {
        let hash = sha256_hex(plaintext);

        let row = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, name, COALESCE(channel_allow, ARRAY[]::text[]) AS channel_allow,
                    COALESCE(model_allow, ARRAY[]::text[]) AS model_allow,
                    enabled, (EXTRACT(EPOCH FROM expires_at) * 1000)::bigint AS expires_at_ms
             FROM api_keys WHERE key_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(row.map(|row| ApiKeyRecord {
            id: row.id,
            name: row.name,
            channel_allow: row.channel_allow,
            model_allow: row.model_allow,
            enabled: row.enabled,
            expires_at_ms: row.expires_at_ms,
        }))
    }

    /// 库里有多少个 Key（用于区分"免鉴权模式"与"配了 Key 必须带"）。
    pub async fn count_api_keys(&self) -> Result<i64, GatewayError> {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys")
            .fetch_one(&self.pool)
            .await
            .map_err(db_error)?;
        Ok(count)
    }

    /// 更新 Key 的最近使用时间。
    ///
    /// 异步写：每个请求都写一次会让热路径多一次往返，而"最近使用"精确到分钟足够。
    pub fn touch_api_key(&self, key_id: i64) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
                .bind(key_id)
                .execute(&pool)
                .await;
        });
    }

    // -----------------------------------------------------------------------
    // 任务执行落库（SPEC 11.2 / 6.2 / 6.3）
    //
    // 分工：run 的**创建**由 Go 管理面做（它要一次性给一批账号建行、拿 batch_id），
    // 网关只负责**回写每条 item 与最终状态** —— 因为只有网关知道每条任务的
    // 真实结果（done / skipped / not_in_window / failed）。
    //
    // 为什么必须落库而不是只放内存：`task_runs` / `task_run_items` 是执行历史的
    // 唯一来源（SPEC 4.4：需要跨重启可见）。只写内存的话，刷新页面就看不到刚跑完的
    // 结果，历史页永远是空的。
    // -----------------------------------------------------------------------

    /// 追加一条任务明细到指定 run。
    ///
    /// `kind` 只能是 `daily` / `task` / `checkin`，由调用方判断
    /// （每日动作 → daily，签到 → checkin，任务清单 → task）。
    pub async fn append_task_item(
        &self,
        run_id: i64,
        code: &str,
        name: &str,
        kind: &str,
        status: &str,
        message: &str,
        earned: u32,
    ) -> Result<(), GatewayError> {
        sqlx::query(
            "INSERT INTO task_run_items (run_id, code, name, kind, status, message, earned)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(run_id)
        .bind(code)
        .bind(name)
        .bind(kind)
        .bind(status)
        .bind(message)
        .bind(earned as i32)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(())
    }

    /// 结束一个 run：写计数、最终状态与错误信息。
    ///
    /// 状态聚合规则见 SPEC 6.2，**必须由这里统一计算**：
    /// `skipped` / `not_in_window` 只进 `skipped_count`，绝不计入 `failed_count`，
    /// 否则一键执行永远是 `partial`（WorkBuddy 有一半任务要桌面端手动做）。
    pub async fn finish_task_run(
        &self,
        run_id: i64,
        done: u32,
        failed: u32,
        skipped: u32,
        earned: u32,
        error: Option<&str>,
    ) -> Result<(), GatewayError> {
        let status = aggregate_run_status(done, failed, error.is_some());

        sqlx::query(
            "UPDATE task_runs SET
                 status = $2,
                 total = $3,
                 done_count = $4,
                 failed_count = $5,
                 skipped_count = $6,
                 earned = $7,
                 error = $8,
                 finished_at = now()
             WHERE id = $1 AND status = 'running'",
        )
        .bind(run_id)
        .bind(status)
        .bind((done + failed + skipped) as i32)
        .bind(done as i32)
        .bind(failed as i32)
        .bind(skipped as i32)
        .bind(earned as i32)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(())
    }

    /// 启动时清理上次进程遗留的 `running` 行（SPEC 11.7）。
    ///
    /// 执行状态活在进程内存里，进程一死就不可能再有人把它写回终态。
    /// 不清理的话这些行会永远显示"执行中"，界面上按钮一直是取消，
    /// 账号也因为"已有 run 在跑"而再也发不起新执行。
    ///
    /// 返回被清理的行数（0 是正常情况，不需要特别提示）。
    pub async fn fail_orphan_task_runs(&self) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "UPDATE task_runs
                SET status = 'failed',
                    error = '进程重启，执行中断',
                    finished_at = now()
              WHERE status = 'running'",
        )
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(result.rows_affected())
    }

    /// 为**定时巡检**建一条 run 行（SPEC 11.8）。
    ///
    /// 手动执行的行由 Go 管理面建（它要拿 batch_id）；巡检没有管理面参与，
    /// 只能由网关自己建。不建行的话巡检结果只写 stderr，
    /// 等于"跑过但查不到" —— 参考项目就是这样，是这条规格要修的缺陷。
    ///
    /// `batch_id` 留 NULL：一次巡检不是一个用户发起的批次，
    /// 硬编一个 uuid 会让"按批次查"多出一堆伪批次。
    pub async fn start_scheduled_run(
        &self,
        channel_id: &str,
        uid: &str,
        scope: &str,
        total: i32,
    ) -> Result<i64, GatewayError> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO task_runs
                 (batch_id, channel_id, account_uid, scope, status, trigger, total)
             VALUES (NULL, $1, $2, $3, 'running', 'schedule', $4)
             RETURNING id",
        )
        .bind(channel_id)
        .bind(uid)
        .bind(scope)
        .bind(total)
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(id)
    }

    // -----------------------------------------------------------------------
    // 运行策略（SPEC 5.7 / 11.8）
    //
    // 这些键由管理面的 `/settings` 页写入 `settings` 表。网关启动时读一次，
    // 巡检循环每轮重读 —— 用户在界面上改了「关掉定时巡检」，30 秒内就该生效，
    // 而不是"改了没用，得重启网关"。所以读设置不能只在 initialize 里做一次。
    // -----------------------------------------------------------------------

    /// 读运行策略。缺失的键走默认值，读失败也走默认值（不该让网关起不来）。
    pub async fn load_policy(&self) -> GatewayPolicy {
        let mut policy = GatewayPolicy::default();

        let rows = match sqlx::query_as::<_, SettingRow>("SELECT key, value FROM settings")
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                // 表不存在/连接抖动都走默认值：默认值是"能干活的那一档"。
                eprintln!("[gateway] 读取运行策略失败（用默认值）：{error}");
                return policy;
            }
        };

        for row in rows {
            match row.key.as_str() {
                "task.schedule_enabled" => {
                    if let Some(value) = row.value.as_bool() {
                        policy.schedule_enabled = value;
                    }
                }
                "task.schedule_hours" => {
                    if let Some(values) = row.value.as_array() {
                        let hours: Vec<u32> = values
                            .iter()
                            .filter_map(|value| value.as_u64())
                            .filter(|hour| *hour < 24)
                            .map(|hour| hour as u32)
                            .collect();
                        if !hours.is_empty() {
                            policy.schedule_hours = hours;
                        }
                    }
                }
                "task.per_account_gap_ms" => {
                    if let Some(value) = row.value.as_u64() {
                        policy.per_account_gap_ms = value;
                    }
                }
                "task.daily_gap_ms" => {
                    if let Some(value) = row.value.as_u64() {
                        policy.daily_gap_ms = value;
                    }
                }
                "task.report_gap_ms" => {
                    if let Some(value) = row.value.as_u64() {
                        policy.report_gap_ms = value;
                    }
                }
                "task.retention_days" => {
                    if let Some(value) = row.value.as_u64() {
                        // 夹到 7–365：界面上有校验，但库里的值可能是脚本直接写的。
                        policy.task_retention_days = value.clamp(7, 365);
                    }
                }
                "log.retention_days" => {
                    if let Some(value) = row.value.as_u64() {
                        policy.log_retention_days = value.clamp(1, 3650);
                    }
                }
                "notify.webhook_url" => {
                    if let Some(value) = row.value.as_str() {
                        policy.webhook_url = value.trim().to_string();
                    }
                }
                "probe.enabled" => {
                    if let Some(value) = row.value.as_bool() {
                        policy.probe_enabled = value;
                    }
                }
                "probe.interval_minutes" => {
                    if let Some(value) = row.value.as_u64() {
                        // 下限 5 分钟：探活是自费动作，间隔太短等于自己烧额度。
                        // 上限 24 小时：再长就失去"早发现"的意义了。
                        policy.probe_interval_minutes = value.clamp(5, 1440) as u32;
                    }
                }
                _ => {}
            }
        }

        policy
    }

    /// 按保留天数清理任务执行历史（SPEC 11.8）。
    ///
    /// 明细靠 `ON DELETE CASCADE` 跟着删，不用单独一条语句 —— 单独删要处理
    /// "明细比 run 先删完"的中间态，级联没有这个问题。
    ///
    /// 返回被删掉的 run 行数。
    pub async fn purge_task_history(&self, days: u64) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "DELETE FROM task_runs
              WHERE started_at < now() - make_interval(days => $1::int)",
        )
        .bind(days as i32)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(result.rows_affected())
    }

    /// 按保留天数清理用量日志（SPEC 11.8 / 17）。
    ///
    /// **只删超期行**：`usage_logs` 是流水，"不可变"指的是不改已有行，
    /// 不是永远留着。保留天数就是它唯一的生命周期来源。
    pub async fn purge_usage_logs(&self, days: u64) -> Result<u64, GatewayError> {
        let result = sqlx::query(
            "DELETE FROM usage_logs
              WHERE ts < now() - make_interval(days => $1::int)",
        )
        .bind(days as i32)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(result.rows_affected())
    }
}

/// 网关的运行策略（从 `settings` 表读）。
#[derive(Debug, Clone)]
pub struct GatewayPolicy {
    /// 定时巡检开关。关掉后只在启动时跑一轮，之后不再自动补跑。
    pub schedule_enabled: bool,
    /// 每日动作的补跑整点档。
    pub schedule_hours: Vec<u32>,
    /// 账号之间的间隔。
    pub per_account_gap_ms: u64,
    /// 同一账号内动作之间的间隔。
    pub daily_gap_ms: u64,
    /// 任务事件上报之间的间隔。
    pub report_gap_ms: u64,
    /// 任务执行历史保留天数。
    pub task_retention_days: u64,
    /// 用量日志保留天数。
    pub log_retention_days: u64,
    /// 告警 webhook 地址（空 = 不发通知，SPEC 9.13）。
    pub webhook_url: String,
    /// 定时探活开关。
    pub probe_enabled: bool,
    /// 探活间隔（分钟），下限 5。
    pub probe_interval_minutes: u32,
}

impl Default for GatewayPolicy {
    fn default() -> Self {
        // 默认值与 `001_init.sql` 里插入的行保持一致：库还没建好时行为不变。
        Self {
            schedule_enabled: true,
            schedule_hours: vec![9, 21],
            per_account_gap_ms: 800,
            daily_gap_ms: 1_200,
            report_gap_ms: 1_100,
            task_retention_days: 90,
            log_retention_days: 30,
            webhook_url: String::new(),
            // 探活默认开、30 分钟一次：故障早发现的价值大于那点额度消耗
            // （每次只发一句 hello，且优先选免费模型）。
            probe_enabled: true,
            probe_interval_minutes: 30,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SettingRow {
    key: String,
    value: serde_json::Value,
}

/// 按 SPEC 6.2 聚合 run 的最终状态。
///
/// ```text
/// failed == 0 && done > 0    → done
/// failed > 0  && done > 0    → partial
/// failed > 0  && done == 0   → failed
/// 两者都为 0（全是 skipped）  → done
/// ```
///
/// `hard_error`（渠道整体不可用，例如网关拿不到任务清单）直接判 `failed`，
/// 即使前面已经做成了几条 —— 因为这次执行是被中途打断的，不是正常收尾。
fn aggregate_run_status(done: u32, failed: u32, hard_error: bool) -> &'static str {
    if hard_error {
        return "failed";
    }
    match (done, failed) {
        (0, 0) => "done",
        (_, 0) => "done",
        (0, _) => "failed",
        _ => "partial",
    }
}

/// API Key 的权限记录（网关鉴权用）。
#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: i64,
    pub name: String,
    pub channel_allow: Vec<String>,
    pub model_allow: Vec<String>,
    pub enabled: bool,
    pub expires_at_ms: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: i64,
    name: String,
    channel_allow: Vec<String>,
    model_allow: Vec<String>,
    enabled: bool,
    expires_at_ms: Option<i64>,
}

/// SHA-256 十六进制摘要。
fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();

    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[derive(sqlx::FromRow)]
struct AffinityRow {
    key: String,
    channel_id: String,
    uid: String,
    upstream_model: String,
    expires_at_ms: i64,
    hits: i32,
}

async fn insert_usage(pool: &PgPool, record: &UsageRecord) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO usage_logs (
            request_id, key_id, channel_id, account_uid, protocol,
            model, upstream_model, stream, status, is_rejection, attempts,
            prompt_tokens, completion_tokens, reasoning_tokens, cached_tokens,
            total_tokens, usage_estimated, upstream_cost, billed_cost,
            input_price, output_price, cache_read_price, model_ratio, channel_ratio,
            ttft_ms, total_ms, effort, error_detail
         ) VALUES (
            $1, $2, $3, $4, $5,
            $6, $7, $8, $9, $10, $11,
            $12, $13, $14, $15,
            $16, $17, $18, $19,
            $20, $21, $22, $23, $24,
            $25, $26, $27, $28
         )",
    )
    .bind(&record.request_id)
    .bind(record.key_id)
    .bind(&record.channel_id)
    .bind(&record.account_uid)
    .bind(&record.protocol)
    .bind(&record.model)
    .bind(&record.upstream_model)
    .bind(record.stream)
    .bind(&record.status)
    .bind(record.is_rejection)
    .bind(record.attempts as i32)
    .bind(record.usage.prompt_tokens as i64)
    .bind(record.usage.completion_tokens as i64)
    .bind(record.usage.reasoning_tokens as i64)
    .bind(record.usage.cached_tokens as i64)
    .bind(record.usage.total_tokens as i64)
    .bind(record.estimated)
    .bind(record.upstream_cost)
    .bind(record.billed_cost)
    // 价格快照：拿不到价格时（`price_snapshot` 为 None）全部存 NULL，
    // 界面据此显示"这条没有价格快照"，而不是编一个 0 出来。
    .bind(record.price_snapshot.map(|snapshot| snapshot.input))
    .bind(record.price_snapshot.map(|snapshot| snapshot.output))
    .bind(record.price_snapshot.map(|snapshot| snapshot.cache_read))
    .bind(record.price_snapshot.map(|snapshot| snapshot.model_ratio))
    .bind(record.price_snapshot.map(|snapshot| snapshot.channel_ratio))
    .bind(record.ttft_ms.map(|value| value as i32))
    .bind(record.total_ms as i32)
    .bind(&record.effort)
    .bind(&record.error_detail)
    .execute(pool)
    .await?;

    // 内容留存：只在开关打开时写。
    //
    // 用 `(SELECT id FROM usage_logs WHERE request_id = $1 LIMIT 1)` 而不是
    // `lastval()`：批量写入时 lastval 可能已经不是这一条了。
    // request_id 不是唯一键（重试会产生多条），所以取任意一条 ——
    // payload 与"这次请求"绑定，具体挂在哪条日志上不影响回放。
    if let Some(payload) = record.payload.as_ref() {
        let bytes = payload.request.len() + payload.response.len();
        // `$2::jsonb` 显式转换：sqlx 绑 `&String` 时推断成 text，
        // 直接写进 jsonb 列会报"column is of type jsonb but expression is of type text"。
        // 这个错误之前被 `let _ =` 吞掉了（见下面的注释），所以表现为"打开了却没有数据"。
        if let Err(error) = sqlx::query(
            "INSERT INTO usage_payloads (usage_id, request, response, bytes)
             SELECT id, $2::jsonb, $3::jsonb, $4
             FROM usage_logs WHERE request_id = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(&record.request_id)
        .bind(&payload.request)
        .bind(&payload.response)
        .bind(bytes as i32)
        .execute(pool)
        .await
        {
            // 仍然不让它影响主流程，但**必须打日志**：
            // 静默失败会让"功能打开了却没数据"变成一个查不出原因的谜。
            eprintln!(
                "[gateway] 内容留存写入失败 trace={}：{error}",
                record.request_id
            );
        }
    }

    Ok(())
}

fn db_error(error: sqlx::Error) -> GatewayError {
    GatewayError::internal(format!("数据库操作失败：{error}"))
}

#[derive(sqlx::FromRow)]
struct ChannelRow {
    id: String,
    name: String,
    adapter: String,
    enabled: bool,
    priority: i32,
    weight: i32,
    options: serde_json::Value,
    channel_ratio: f64,
    model_allow: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    /*
      可空：账号可以先建、后分配分组。

      读出来转成空串（而不是 Option 一路传穿）：
      空串 key 的账号不会命中任何真实 channel，天然不参与调度，
      而其余代码继续按 String 处理，改动面最小。
    */
    channel_id: Option<String>,
    uid: String,
    nickname: String,
    enabled: bool,
    priority_override: Option<i32>,
    weight_override: Option<i32>,
    max_inflight: Option<i32>,
    credential_cipher: Option<Vec<u8>>,
    model_allow: Option<Vec<String>>,
    model_rename: Option<serde_json::Value>,
    /// 账号级上游基址（空 = 用适配器默认）。
    base_url: String,
    /// 账号级上游协议。
    protocol: String,
    /// 账号所属平台（决定选哪个适配器）。
    platform: String,
    /// 适配器私有参数（realm / outbound_identity 等）。
    options: serde_json::Value,
    // ---- 代理（LEFT JOIN，未绑定时全为 NULL）----
    proxy_id: Option<i64>,
    proxy_name: Option<String>,
    proxy_protocol: Option<String>,
    proxy_host: Option<String>,
    proxy_port: Option<i32>,
    proxy_username: Option<String>,
    proxy_password_enc: Option<String>,
    proxy_fallback: Option<String>,
    proxy_enabled: Option<bool>,
}

/// 把渠道与账号记录转成网关的运行时配置。
pub fn to_account_config(record: &AccountRecord) -> AccountConfig {
    AccountConfig {
        channel_id: record.channel_id.clone(),
        uid: record.uid.clone(),
        nickname: record.nickname.clone(),
        enabled: record.enabled,
        priority_override: record.priority_override,
        weight_override: record.weight_override,
        max_inflight: record.max_inflight,
        model_allow: record.model_allow.clone(),
        model_rename: record.model_rename.clone().into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::aggregate_run_status;

    /// SPEC 6.2 的四种组合，逐条钉死。
    ///
    /// 这几行是"红点是否可信"的根：把 skipped 算进 failed，一键执行永远是
    /// partial，用户就不再看这个标志了 —— 那等于没有告警。
    #[test]
    fn run_status_follows_spec_6_2() {
        // 全部成功
        assert_eq!(aggregate_run_status(5, 0, false), "done");
        // 成功 + 失败 → 部分完成
        assert_eq!(aggregate_run_status(5, 2, false), "partial");
        // 全失败
        assert_eq!(aggregate_run_status(0, 3, false), "failed");
        // 全是 skipped（done 与 failed 都是 0）→ done，不是 failed
        assert_eq!(aggregate_run_status(0, 0, false), "done");
    }

    /// 渠道级硬失败（拿不到清单）即使前面做成了几条，也判 failed。
    #[test]
    fn hard_error_forces_failed() {
        assert_eq!(aggregate_run_status(3, 0, true), "failed");
        assert_eq!(aggregate_run_status(0, 0, true), "failed");
    }
}
