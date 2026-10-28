# relayAI 迁移方案：从 Tauri 桌面网关到 Go + Rust 双服务

> 状态：**方案稿 v0.1**，依据 `E:\code\dynamic-island` 实际代码盘点，尚未动手改代码。
> 前置文档：[product-plan.md](./product-plan.md)（产品与功能规划 v0.2）。

---

## 0. 一句话结论

参考项目比预想完整得多：**三协议入口、账号池调度、熔断冷却、会话亲和、凭据加密、用量统计全部已实现**，生产代码 21,044 行。

真正需要迁移的不是"逻辑"，而是**三处外壳**：Tauri 运行时、SQLite、Tauri 命令通道。
核心网关（`core` / `channels` / `protocol` / `server` / `transport`）**零 Tauri 依赖**，可以整块搬走。

---

## 1. 现状盘点

### 1.1 规模

| 模块 | 生产行数 | 说明 |
| --- | --- | --- |
| `channels/` | 15,511 | workbuddy / trae / custom / codearts / fake / openai_compat |
| `core/` | 3,823 | 调度、选号、矩阵、亲和、凭据刷新 |
| `protocol/` | 2,584 | 三协议 decode + encode + CIR |
| `server/` | 1,754 | axum 路由 + 三入口 handler + SSE |
| `commands/` | 2,467 | **44 个 Tauri 命令**（面板 API，需重写） |
| `storage.rs` | 890 | SQLite（需重写） |
| `state.rs` | 495 | 共享状态 |
| `config.rs` | 495 | JSON 配置 |
| 其余 | ~1,000 | errors / security / transport / timeutil / ops |
| **合计** | **21,044** | 另有 8,228 行测试 |

### 1.2 已实现的能力（不需要重做）

- **三协议入口**：`/v1/chat/completions`、`/v1/completions`、`/v1/messages`、`/v1/messages/count_tokens`、`/v1/responses`、`/v1/models`
- **协议互转**：统一 CIR（`CanonicalRequest` / `CanonicalEvent`），三向 decode/encode 齐备
- **调度**：候选矩阵 → 亲和短路 → 优先级分层 → 打分加权随机 → 重试排除
- **错误语义**：`ErrorKind` + `ErrorScope`（请求级 / 账号级 / 账号-模型级），适配器只翻译语义
- **冷却**：认证失败、软限流、熔断器三级退避；**模型级冷却**不牵连同账号其他模型
- **会话亲和**：绑定到账号级保上游前缀缓存，支持 prefer/strict/off
- **凭据**：AES-256-GCM 信封 + 主密钥进系统凭据库；刷新按账号串行（防 refresh_token 轮换作废）
- **用量**：单写线程批量落库、CJK 感知估算、`estimated` 标记
- **渠道**：WorkBuddy（国内/国际 + 三套出站身份 + 指纹清洗）、Trae（两段式会话）、自定义供应商（OpenAI/Anthropic/Responses 三种上游协议）

### 1.3 Tauri 耦合面（精确到文件）

只有 **4 处**，且都不在核心逻辑里：

| 位置 | 内容 | 迁移动作 |
| --- | --- | --- |
| `gateway/mod.rs` | `setup(app: &AppHandle)`、`app.path().app_data_dir()`、`tauri::async_runtime::spawn` | 改为 `main.rs` 里读环境变量 + `tokio::spawn` |
| `gateway/ops.rs` | 1 处 `tauri::async_runtime::spawn` | 改 `tokio::spawn` |
| `gateway/commands/` | 44 个 `#[tauri::command]` + `State<'_, SharedGateway>`（`mod.rs` 41 + `providers.rs` 3） | **整层重写为 axum admin 路由** |
| `gateway/storage.rs` + `security.rs` | `SecretStore` 走 `keyring` 取主密钥 | 改为环境变量 `RELAYAI_MASTER_KEY` |

另有两处 `windows` crate：

| 位置 | 用途 | 迁移动作 |
| --- | --- | --- |
| `timeutil.rs` | `GetLocalTime()` 取本地日期/小时 | 服务器上直接用 `chrono::Local`，删掉 cfg 分支 |
| `channels/workbuddy/tasks.rs` | `in_night_window()` 判断 23:00–08:00 | 同上 |

> 结论：**去 Tauri 化本身是低风险工作**。真正的工作量在 `commands/`（2,467 行面板 API）和 `storage.rs`（890 行 SQLite → PG）。

---

## 2. 双服务的边界怎么划

你选了「Go 管管理面 + Rust 跑网关」。这条路的**唯一难点是数据归属**，必须先定死，否则会变成两套真相。

### 方案对比

| 方案 | 数据归属 | 优点 | 缺点 |
| --- | --- | --- | --- |
| **A. Rust 独占** | Rust 持有 config.json + SQLite，Go 通过 admin HTTP 读写 | 改动最小，Rust 侧几乎不用动存储 | Go 沦为纯转发，PG 里只有 UI 偏好；查询统计受限于 Rust 提供的接口 |
| **B. PG 单一真相** | 两者都连 PG；Rust 启动加载配置，写入用 PG | 统计/筛选可以在 Go 侧自由写 SQL | Rust 的 `storage.rs` 要全量重写；Rust 侧配置热更新要加通知机制 |
| **C. 混合** | 配置与账号在 PG（Go 写），运行态在 Rust 内存，用量 Rust 直写 PG | 各取所长 | 需要明确"哪些字段是配置、哪些是运行态"，边界要写进契约 |

**推荐 C**，理由：

1. **配置类**（渠道、账号、模型映射、倍率、密钥）变更频率低、需要人工编辑 → 归 PG，Go 做 CRUD，天然有事务与并发控制。
2. **运行态**（冷却剩余、在飞并发、亲和绑定、最近决策）是秒级变化的 → 留在 Rust 内存，绝不能进库（每请求一次写库会把性能打没）。
3. **用量**（`usage_logs`）写入频率最高 → Rust 直写 PG，用批量 COPY；Go 只读做统计。

这样 Go 不需要知道任何渠道细节，Rust 也不需要实现任何列表分页。

### 需要新增的 Rust 内部接口

Rust 侧暴露一组**只绑 127.0.0.1 + 共享密钥**的 admin 路由，供 Go 调用那些「必须贴着适配器才能做」的操作：

```
POST /internal/channels/:id/probe        # 真实探活
POST /accounts/:key/refresh              # 强制刷新凭据（force=true）
POST /accounts/:key/login/start|poll|complete
POST /accounts/:key/quota                # 额度快照
POST /accounts/:key/tasks/run            # 运营任务（workbuddy）
POST /accounts/:key/daily/run            # 每日动作
GET  /internal/scheduler                 # 候选矩阵 + 决策环形缓冲
POST /internal/matrix/refresh
```

这些对应现有 `commands/` 里 44 个命令中的 ~22 个；剩下的 ~22 个（账号增删改、渠道增删改、用量查询）**直接在 Go 里对 PG 做 CRUD**，不需要经过 Rust。

---

## 3. 存储迁移：SQLite → PostgreSQL

### 3.1 表映射

| 现 SQLite | 目标 PG | 备注 |
| --- | --- | --- |
| `accounts(channel_id, uid, ...)` | `accounts` 加自增 `id`，`(channel_id, uid)` 保留唯一索引 | 主键语义变了：运行态仍用复合键寻址 |
| `accounts.credential TEXT` | `credential_cipher BYTEA` | 已是 AES-GCM base64，换成 `bytea` 省一次编码 |
| `accounts.model_allow TEXT`（JSON 数组） | `model_allow TEXT[]` | 可以直接用数组操作符查询 |
| `accounts.model_rename TEXT`（JSON 对象） | `model_rename JSONB` | |
| `usage` | `usage_logs` | 见下 |
| `affinity` | **不进 PG** | 纯运行态，重启可重建；进了反而要处理 TTL 清理 |

### 3.2 `usage_logs` 的关键改动

```sql
CREATE TABLE usage_logs (
    id                BIGSERIAL PRIMARY KEY,
    ts                TIMESTAMPTZ NOT NULL,
    model             TEXT NOT NULL,
    upstream_model    TEXT,
    channel_id        TEXT NOT NULL,
    account_uid       TEXT NOT NULL,
    status            TEXT NOT NULL,          -- completed | failed | client_aborted | upstream_aborted | rejected
    stream            BOOLEAN NOT NULL,
    attempts          INT NOT NULL,
    prompt_tokens     BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    reasoning_tokens  BIGINT NOT NULL DEFAULT 0,
    cached_tokens     BIGINT NOT NULL DEFAULT 0,
    total_tokens      BIGINT NOT NULL DEFAULT 0,
    credit            NUMERIC(14,6),
    ttft_ms           INT,
    total_ms          INT NOT NULL,
    effort            TEXT,
    estimated         BOOLEAN NOT NULL DEFAULT false,
    error_detail      TEXT
);

CREATE INDEX usage_ts      ON usage_logs (ts DESC);
CREATE INDEX usage_channel ON usage_logs (channel_id, ts DESC);
CREATE INDEX usage_model   ON usage_logs (model, ts DESC);
```

三点注意：

1. **`ts` 用 `TIMESTAMPTZ` 而不是 epoch 毫秒整数**。现在代码里到处是 `now_ms()` 与手工算本地时区（`local_utc_offset_secs()` 那段"用本地小时减 UTC 小时反推偏移"的逻辑非常容易在跨时区/夏令时下算错）。换成 `timestamptz` 后，"今日"直接写 `ts >= date_trunc('day', now())`。
2. **`BIGSERIAL` 而不是保留 `ts` 做排序键**：同一毫秒内的多条记录目前无法稳定分页。
3. **批量写入**：沿用现在的"单写线程 + 批量事务"思路，改成攒 200 条或 1 秒 `COPY` 一次。

### 3.3 主密钥

现在：`keyring` → Windows 凭据管理器 → `gateway/master-key`。
迁移后：环境变量 `RELAYAI_MASTER_KEY`（32 字节 base64），启动时校验长度，缺失直接拒绝启动。

> **不能**沿用 keyring：服务器上没有交互式凭据库，容器里更是没有。

---

## 4. 管理面：44 个 Tauri 命令 → REST

按归属拆成两组：

### 4.1 直接进 Go（对 PG 做 CRUD，不经过 Rust）

| 现命令 | 目标路由 |
| --- | --- |
| `gateway_list_channels` / `gateway_update_channel` / `gateway_delete_channel` | `GET/PATCH/DELETE /api/v1/channels` |
| `gateway_list_accounts` / `gateway_add_account` / `gateway_upsert_account_config` | `GET/POST/PATCH /api/v1/accounts` |
| `gateway_set_account_enabled` / `gateway_set_account_max_inflight` / `gateway_set_account_profile` / `gateway_set_account_models` / `gateway_delete_account` | `PATCH/DELETE /api/v1/accounts/:key` |
| `gateway_list_routes` / `gateway_set_overrides` | `GET/PUT /api/v1/routes` |
| `gateway_usage_summary` / `gateway_usage_summary_range` / `gateway_usage_series` / `gateway_usage_series_range` / `gateway_recent_usage` | `GET /api/v1/usage*` |
| `gateway_set_runtime`（端口/API Key/内网） | `PATCH /api/v1/settings` |
| `gateway_paths` | 不需要（桌面端专有） |

### 4.2 必须走 Rust（贴着适配器）

| 现命令 | 目标路由（Rust 内部） |
| --- | --- |
| `gateway_login_start` / `gateway_login_poll` / `gateway_login_complete` | `POST /internal/accounts/:key/login/*` |
| `gateway_refresh_account` | `POST /internal/accounts/:key/refresh` |
| `gateway_probe_account` / `gateway_probe_models` | `POST /internal/accounts/:key/probe` |
| `gateway_fetch_quota` / `gateway_fetch_models` / `gateway_diagnose_channel` | `GET /internal/...` |
| `gateway_list_tasks` / `gateway_run_tasks` / `gateway_task_progress` / `gateway_claim_task` | `POST /internal/accounts/:key/tasks/*` |
| `gateway_list_daily` / `gateway_run_daily` / `gateway_checkin_account` | `POST /internal/accounts/:key/daily/*` |
| `gateway_reset_account_state` / `gateway_clear_affinity` | `POST /internal/state/*` |
| `gateway_status` / `gateway_scheduler_snapshot` / `gateway_recent_decisions` / `gateway_refresh_matrix` | `GET /internal/status` 等 |

> `gateway_checkin_account` 和任务相关的一组是 **WorkBuddy 专有运营能力**（签到、猫猫旅行、成长任务）。如果 relayAI 只做"反代"，这批可以先不迁——它们对应 `/channels/:id` 详情页里的一个可选 Tab。

---

## 5. 盘点中发现的三处问题（迁移前应先修）

### 5.1 用量统计的排除条件已失效（真 bug）

`server/mod.rs::record_rejection` 现在给拒绝行打的前缀是 `(上游失败 …)` / `(无可用账号 …)` / `(限流或并发 …)` / `(请求被拒 …)`：

```rust
model: format!("({} {path})", match error.kind { ... })
```

但 `storage.rs` 的三处统计仍在排除旧前缀：

```sql
AND model NOT LIKE '(未进入调度%'
```

**结果：被拒绝的请求现在会被计入总量、按渠道分组、按模型分组。** 面板上会多出一个 `channel_id = "-"` 的分组，且成功率被拉低。

修法二选一：给拒绝行加一个显式布尔列（`is_rejection`），或统一前缀常量。**推荐前者**——靠字符串前缀做语义判断，下次改名还会再坏一次。

### 5.2 `commands/head_restore.rs` 是未编译的残留

- 文件是 **UTF-16LE + BOM**，全仓唯一一个非 UTF-8 源文件
- 内含 37 个命令，是 `mod.rs`（41 个）的**子集**，缺 `gateway_login_complete` / `gateway_set_account_profile` / `gateway_set_account_models` / `gateway_diagnose_channel`，也不含 `providers.rs` 那 3 个（`gateway_fetch_models` / `gateway_save_custom_provider` / `gateway_delete_channel`）
- 全仓**没有任何 `mod head_restore;` 引用** → 未参与编译

迁移时直接丢弃，不要顺手带走。

### 5.3 时区计算过于手工

`storage.rs` 的 `local_utc_offset_secs()` 用"本地小时 − UTC 小时"反推偏移，再夹回 ±12h。这个算法在**半小时时区**（印度 +5:30、尼泊尔 +5:45）和**夏令时切换当天**都会算错。迁到 PG 的 `timestamptz` 后这段可以整块删掉。

---

## 6. 修订后的里程碑

| 阶段 | 交付 | 验收 |
| --- | --- | --- |
| **M0 清理** | 修 5.1 统计 bug、丢弃 head_restore.rs | 拒绝行不再污染统计 |
| **M1 骨架** | relayAI 前端四组导航 + 页面壳 | 页面全可达、响应式正常 |
| **M2 Rust 去 Tauri** | 拆 `gateway/` 成独立 crate，`main.rs` 起 axum；主密钥走环境变量 | 二进制能独立跑，`/v1/chat/completions` curl 通 |
| **M3 PG 迁移** | `storage.rs` 重写为 sqlx；账号/渠道/用量落 PG；`timestamptz` | 老 SQLite 数据能导入，统计口径对齐 |
| **M4 Go 管理面** | go-zero CRUD + Rust admin 代理 + 前端接上 | 前端能增删改渠道/账号，能看用量 |
| **M5 三协议联调** | 三种客户端各跑一次工具调用 | Chat Completions / Messages / Responses 全通 |
| **M6 任务模块** | 任务一级菜单 + 渠道二级菜单；PG 化 run 记录；批量/取消/历史 | 一个渠道 3 个账号能串行跑完，逐条进度可见，可取消 |
| **M7 观测** | 倍率计费、渠道状态页、告警 Webhook | 掉号 1 分钟内收到通知 |

> 相比 product-plan.md 的 M4「三协议」——**这块已经不用做了**，参考项目里三种入口与互转都已实现并有测试覆盖（含"Codex 接网关报 `summaryParts` undefined"这类真实踩坑修复）。

---

## 7. 待确认

1. **WorkBuddy 的运营能力（签到 / 任务 / 每日动作 / 猫猫旅行）要不要一起迁？**
   **已确认：要迁**，并且提升为独立一级菜单 `任务`，二级菜单是"有能力的渠道"，不再挂在账号行下面。
   详见 [task-module.md](./task-module.md)——注意这块**不能原样搬**：现有 `task_runs` 是内存 HashMap 且从不清理（泄漏），也没有渠道级批量、取消、历史。
2. **`codearts` 渠道还要不要？** 现在适配器已从注册表摘掉（上游限制 3 会话并发 + `tool_choice` 方言），926 行代码是死重。建议直接删。
3. **数据归属按方案 C 走，确认吗？** 这决定 Rust 侧要不要重写存储层，也决定 Go 能不能自由写统计 SQL。
4. **是否保留"允许局域网访问"？** 桌面端默认只绑回环；做成服务后，绑定地址和鉴权策略要重新定（现在 `api_keys` 为空即"免鉴权"，服务端不能沿用这个默认）。
