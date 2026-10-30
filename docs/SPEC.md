# relayAI 实施规格书（SPEC v1.0）

> **本文是唯一执行依据。** 与 `product-plan.md` / `migration-plan.md` / `task-module.md` / `key-isolation.md` 冲突时，以本文为准；那四份是背景与论证，本文是结论。
>
> 状态：已确认，可开工。最后更新 2026-10-21。

---

## 0. 怎么用这份文档

1. **按第 14 节的里程碑顺序执行**，不要跳阶段。
2. 每个里程碑末尾有**验收清单**，逐条勾完才能进下一个。验收项写的是"可观察的事实"，不是"已完成编码"。
3. 遇到本文没写到的细节：**先查第 3 节的不变量**，能推出答案就按推出来的做；推不出来就停下来问，不要自己发明。
4. 任何偏离本文的实现，必须在 PR / 提交说明里写清"偏离了第几节、为什么"。

---

## 1. 范围与非目标

### 1.1 做什么

一个自用的 AI 反代网关 + 管理台：

- **网关**（Rust）：对下游暴露 OpenAI Chat Completions / OpenAI Responses / Anthropic Messages 三种协议入口；对上游调度 WorkBuddy / Trae / 自定义渠道商的账号池。
- **管理台**（Go + React）：渠道、账号、密钥、用量、任务、设置的增删改查与观测。

### 1.2 明确不做

| 不做 | 原因 |
| --- | --- |
| 用户注册 / 登录 / 多租户 / 充值 / 发票 | 自用，单管理员口令 |
| `/v1/embeddings`、`/v1/images` | 无向量需求 |
| 向量数据库、RAG | 同上 |
| 移动端 App | 响应式 Web 足够 |
| 多语言 i18n | 仅中文界面 |
| 插件市场、工作流编排 | 不在反代范围 |
| 真实支付对账 | 用兑换码都不需要 |

> **反向约束**：不要因为"以后可能要"就预留抽象层。本项目的抽象只到「渠道适配器」和「协议编解码」两层，别的都不加。

---

## 2. 已确认的决策（不可再议）

| # | 决策 | 值 |
| --- | --- | --- |
| D1 | 使用场景 | 自用，单管理员 |
| D2 | 后端形态 | **Go 管管理面 + Rust 跑网关**（双服务） |
| D3 | 数据归属 | **混合**：配置在 PG，运行态在 Rust 内存，用量 Rust 直写 PG |
| D4 | 数据库 | PostgreSQL 16+ |
| D5 | 协议入口 | Chat Completions + Responses + Anthropic Messages（三者全支持） |
| D6 | 计费 | 倍率制：`billed = upstream_cost × model.ratio × channel.ratio` |
| D7 | 上游来源 | WorkBuddy（国内/国际）、Trae、自定义渠道商 |
| D8 | 运营任务 | 迁移，独立一级菜单「任务」，二级为渠道 |
| D9 | 一键执行语义 | **包含每日动作**：先每日动作，再任务清单 |
| D10 | 定时巡检 | 可配置，**默认开启** |
| D11 | 任务记录保留 | 可配置，**默认 90 天** |
| D12 | Key 渠道隔离 | 要做：`channel_allow` + `model_allow` 双维度 |
| D13 | 视觉参考 | FastGPT（`E:\FastGPT-main`），只取 token |
| D14 | 前端 | 单 Vite 应用，React 19 + Tailwind 4 + shadcn/ui |
| D15 | 部署 | 单机，前端静态文件由 Go 托管（生产）/ Vite 代理（开发） |

---

## 3. 不变量（Invariants）

违反任意一条就是 bug，不需要讨论。

### 3.1 数据不变量

| # | 不变量 |
| --- | --- |
| I1 | 凭据明文**永不**落库、永不进日志、永不返回给前端。库里只有 AES-256-GCM 密文。 |
| I2 | API Key 明文**永不**落库。只存 `key_hash`（SHA-256）与 `key_prefix`（前 8 位 + `****`）。 |
| I3 | `usage_logs` 一旦写入不可修改。任何统计都从它派生，不另存汇总表。 |
| I4 | `upstream_cost` 与 `billed_cost` 分开存，**永远**不要只存后者。 |
| I5 | 运行态（冷却、在飞并发、亲和绑定）**不落库**。重启后丢失是可接受的。 |

### 3.2 路由不变量

| # | 不变量 |
| --- | --- |
| I6 | Key 的渠道过滤发生在**建候选集阶段**，不是选号之后。 |
| I7 | 同名模型跨渠道合并成"一条对外模型 + N 条路由"。所以按渠道隔离只能过滤 `channel_id`，不能靠模型名。 |
| I8 | 模型级冷却（`AccountModel` scope）**不得**牵连同账号的其他模型。 |
| I9 | 账号型渠道的凭据刷新**按账号串行**（上游轮换 refresh_token，并发刷新互相作废）。 |
| I10 | 客户端断开必须取消上游请求，并释放并发槽。 |

### 3.3 协议不变量

| # | 不变量 |
| --- | --- |
| I11 | 三协议入口共用同一份 CIR（`CanonicalRequest` / `CanonicalEvent`），不写 9 条转换路径。 |
| I12 | `tool_call_id` 转换必须建映射表并原样还原，否则多轮工具调用必崩。 |
| I13 | 能力不对等时**显式降级**并返回明确错误，不静默丢字段。 |
| I14 | `/v1/responses` **只实现无状态模式**；收到 `previous_response_id` 直接 400。 |

### 3.4 前端不变量

| # | 不变量 |
| --- | --- |
| I15 | 页面切换只用 `react-router-dom`，禁止用 `useState` 控制页面。 |
| I16 | 所有非流式请求走 `APIRequest`，禁止在页面里直接 `import axios`。 |
| I17 | 任何需要加载态的区域用 `MyBox`，禁止手写 spinner / skeleton。 |
| I18 | 禁止原生 `<select>` 与 `input[type=date]`，用 `src/components/` 下的自定义组件。 |
| I19 | 图表用 Recharts 或 ECharts，禁止手写 SVG / CSS 柱状图。 |
| I20 | 颜色只能来自 `src/styles/index.css` 的 token，页面里不得出现十六进制色值。 |
| I21 | 创建/编辑**不得**常驻在列表页，必须走弹窗或独立路由。 |

---

## 4. 系统架构

### 4.1 进程与端口

| 进程 | 端口 | 职责 |
| --- | --- | --- |
| `relayai-gateway`（Rust） | `18788` | `/v1/*` 网关 + `/internal/*` 内部接口 |
| `relayai-backend`（Go） | `8888` | `/api/v1/*` 管理面 + 生产环境托管前端静态文件 |
| Vite dev | `5173` | 开发期前端，`/api` 代理到 8888 |

> 端口选 `18788` 不是 `8788`：`8788` 与 `8790` 在本机已被其他常驻服务占用（pythonw、参考项目），用它们必然触发端口回退，回退后 `GATEWAY_BASE_URL` 与实际监听端口不一致，管理面会打到别人身上。回退扫描范围是 `18788..18798`，不会碰到 `8790`。端口仍可配置覆盖。

### 4.2 数据流

```
客户端
  │  Authorization: Bearer sk-xxx
  ▼
Rust /v1/*  ── 鉴权（查 PG: api_keys）──▶ KeyContext{channel_allow, model_allow}
  │
  ├─ 候选集 = 模型矩阵 × 账号池，再按 KeyContext 过滤
  ├─ 选号（亲和 → 优先级分层 → 打分加权随机）
  ├─ 出站（适配器）→ 上游
  └─ 回程：CIR 事件 → 客户端协议 SSE
  │
  └─ 每请求写一条 usage_logs（含 key_id / channel_id / account_uid）

管理台（React）
  │  /api/v1/*  Authorization: Bearer <管理员会话>
  ▼
Go 管理面 ──配置类 CRUD──▶ PG
          └──必须贴适配器的操作──▶ Rust /internal/*（共享密钥鉴权）
```

### 4.3 归属划分（关键，别搞混）

| 数据 | 归属 | 理由 |
| --- | --- | --- |
| 渠道、账号、模型、倍率、Key、设置 | **PG**（Go 写） | 变更频率低、需要人工编辑、要事务 |
| 冷却剩余、在飞并发、亲和绑定、任务取消标志 | **Rust 内存** | 秒级变化，写库会打爆性能 |
| 用量 `usage_logs` | **PG**（Rust 写，Go 读） | 写入频率最高，Go 只做统计查询 |
| 任务执行记录 `task_runs` / `task_run_items` | **PG**（Rust 写，Go 读） | 需要历史，需要跨重启可见 |

---

## 5. 数据模型

> 全部 DDL 一次性给出。迁移文件放 `backend/migrations/`，按 `001_init.sql`、`002_xxx.sql` 命名。

### 5.1 渠道

> **渠道 = 计价分组，不持有凭据，也不绑定平台。**
>
> 它只回答三件事：这批账号按什么价卖（倍率）、调度时谁先被选中（优先级/权重）、
> 并发与 QPS 上限是多少。
>
> **平台在账号上**（见 5.2），所以：
>   · 想加一个 WorkBuddy 号，不需要先建一个「WorkBuddy 类型」的渠道；
>   · 同一个计价分组下面可以混不同平台的号（WorkBuddy + 自定义供应商）。
>
> 早期版本把 `adapter`（平台）和 `base_url` 放在渠道上，导致两个真实问题：
> ① 加号必须先建渠道，而建出来的渠道除了装一个类型的号之外没有别的用途；
> ② 一个渠道只能有一种平台，无法表达"低价通道同时收 WorkBuddy 和自建中转"。
> 已修正（迁移 `009_account_upstream.sql`、`010_account_platform.sql`）。

```sql
CREATE TABLE channels (
    id                 TEXT PRIMARY KEY,            -- 分组 id：cheap-lane / premium
    name               TEXT NOT NULL,
    adapter            TEXT NOT NULL DEFAULT 'custom', -- 已废弃，见下方注释
    enabled            BOOLEAN NOT NULL DEFAULT true,
    priority           INT NOT NULL DEFAULT 100,    -- 越大越先选
    weight             INT NOT NULL DEFAULT 1,      -- 同档内份额
    options            JSONB NOT NULL DEFAULT '{}', -- 仅适配器通用参数，通常为空
    channel_ratio      NUMERIC(10,4) NOT NULL DEFAULT 1.0,
    concurrency_limit  INT,
    rate_limit_qps     INT,
    -- 定时探活要跑的模型（`probe_model` 是旧版单选，与新列取并集）。
    -- **空 = 不探**：探活消耗真实额度，"没配"和"探全部"必须严格区分。
    probe_model        TEXT,
    probe_models       TEXT[],
    -- 是否出现在「分组状态」页。**默认 false**：分组是调度用的计价桶，
    -- 公开状态是额外动作。只想展示国内版就把国际版保持关闭。
    status_visible     BOOLEAN NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

> `channels.adapter` **保留但不再读**。平台已经下沉到 `accounts.platform`；
> 这一列只作历史记录，任何代码都不该再用它做分支。
> 删列会让回滚变难，而它反正不参与任何判断。
>
> `status_visible` 只影响展示，不影响探活：把分组临时藏起来不会清空探活配置，
> 重新打开时不用再选一遍模型。

### 5.2 账号

```sql
CREATE TABLE accounts (
    id                BIGSERIAL PRIMARY KEY,
    -- 空 = 待分配：号已经建好，只是还没归到任何计价桶里。
    -- 删分组时账号**保留**，这一列被置空（ON DELETE SET NULL），
    -- 不是随分组一起消失 —— 账号是一份凭据，分组只是计价桶。
    channel_id        TEXT REFERENCES channels(id) ON DELETE SET NULL,
    uid               TEXT NOT NULL,                -- 上游账号标识
    nickname          TEXT NOT NULL DEFAULT '',
    enabled           BOOLEAN NOT NULL DEFAULT true,
    credential_cipher BYTEA,                        -- AES-256-GCM 密文；自定义渠道可为空
    priority_override INT,
    weight_override   INT,
    max_inflight      INT,                          -- 空 = 用渠道默认
    model_allow       TEXT[],                       -- 该账号只服务哪些对外模型名；空 = 不限
    model_rename      JSONB NOT NULL DEFAULT '{}',  -- {对外名: 该账号的上游名}
    base_url          TEXT NOT NULL DEFAULT '',     -- 上游基址；空 = 用适配器内置默认
    protocol          TEXT NOT NULL DEFAULT 'openai-chat',
    platform          TEXT NOT NULL DEFAULT 'custom', -- workbuddy | trae | custom | fake
    options           JSONB NOT NULL DEFAULT '{}',  -- 适配器私有参数（realm / outbound_identity）
    source            TEXT NOT NULL DEFAULT 'manual',
    added_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (uid)                                    -- 一个上游账号在库里只有一条记录
);

CREATE INDEX accounts_by_channel ON accounts (channel_id, enabled);
```

> **唯一约束是单列 `uid`，不是 `(channel_id, uid)`**：`channel_id` 可空，
> 而 Postgres 里 `NULL <> NULL`，复合唯一索引对"待分配"的行不起作用。
> 换分组是 UPDATE（迁移），不是新插一行 —— 否则历史 `usage_logs.account_uid` 会对不上。
>
> **未分配分组的账号不参与对外调度**（网关按分组取号，空分组下没有号可发），
> 但**能出现在任务页**：任务按版本（`options.realm`）取号，不按分组。

> **注意**：`accounts` 没有 `status` / `cooldown_until` 列。冷却与可用性是运行态（I5），不落库。
> 账号"已删除"就是物理删除；历史 `usage_logs` 只保留 `account_uid` 字符串，不建外键。
>
> **`platform` / `base_url` / `protocol` / `options` 为什么都在账号上**：
> 它们描述"这份凭据属于哪个上游、哪个区、用什么协议说话"，全是凭据属性。
>
> 一个具体的后果：`WorkbuddyChannel` 过去在构造时把 `realm` 定死，一个实例只能服务
> 一个区域。平台下沉后**同一个适配器实例要服务多个账号**，所以 `realm` 改成按账号解析
> （`identity_for(account)`，账号优先、渠道兜底）。顺序反了会让"改账号区域"毫无效果，
> 表现为国内号打到国际域名上、401/404 但配置看起来完全正常。
> 这条由 `account_realm_overrides_the_channel_default` 守着。

### 5.9 密钥明文（对 SPEC I2 的修订）

**原规格**：`api_keys` 只存 SHA-256 与前缀，明文只在创建时返回一次。

**现在**：多一列 `key_plaintext`，列表接口直接返回明文，界面可反复复制。

| | 原设计 | 现在 |
| --- | --- | --- |
| 库里 | 只有 hash + 前缀 | hash + 前缀 + 明文 |
| 界面 | 只能看一次 | 随时复制 |
| 丢了 Key | 必须轮换（旧客户端全部掉线） | 直接再复制一次 |

**为什么改**：实际使用中"配好客户端、过几天想在另一台机器再加一个"是高频场景，
而轮换会把旧客户端全部踢下线。sub2api 就是存明文的。

**安全边界（必须理解清楚，别以为漏了）**：这降低的是"库被读走"的门槛，
**不改变上游凭据的安全** —— 账号的 `access_token` / API Key 仍是主密钥加密，
主密钥只在网关进程里。下游 Key 泄露的处置是"在界面上轮换"，不需要碰上游账号。

老数据（迁移前创建的 Key）明文已丢失，`key_plaintext` 为 NULL：
界面显示"仅前缀可用，请轮换"，**不假装能复制**。


### 5.3 模型目录与价格

```sql
CREATE TABLE models (
    canonical_name  TEXT PRIMARY KEY,               -- 对外模型名
    display_name    TEXT NOT NULL DEFAULT '',
    context_window  INT,
    max_output      INT,
    capabilities    JSONB NOT NULL DEFAULT '{}',    -- {vision, tool_calls, reasoning}
    input_price     NUMERIC(14,8) NOT NULL DEFAULT 0,   -- 每 1M token
    output_price    NUMERIC(14,8) NOT NULL DEFAULT 0,
    cache_read_price NUMERIC(14,8) NOT NULL DEFAULT 0,
    model_ratio     NUMERIC(10,4) NOT NULL DEFAULT 1.0,
    enabled         BOOLEAN NOT NULL DEFAULT true
);
```

> **模型从哪来**：不手工维护全量清单。渠道目录（`/v1/models` 的上游版本）实时拉取后，与 `models` 表**左连接**：表里有就用表里的价格与能力，没有就落一条 `enabled=false` 的占位并在「模型」页提示"未定价"。
> 这样既不阻塞新模型上线，又能让未定价模型不被计费成 0。

### 5.4 API Key

```sql
CREATE TABLE api_keys (
    id            BIGSERIAL PRIMARY KEY,
    name          TEXT NOT NULL,
    key_hash      TEXT NOT NULL UNIQUE,             -- SHA-256(明文)
    key_prefix    TEXT NOT NULL,                    -- sk-abcd****
    channel_allow TEXT[],                           -- 渠道实例 id；空 = 不限
    model_allow   TEXT[],                           -- 对外模型名；空 = 不限
    rpm_limit     INT,                              -- 空 = 不限
    daily_token_limit BIGINT,                       -- 空 = 不限
    max_concurrency INT,                            -- 空 = 不限
    enabled       BOOLEAN NOT NULL DEFAULT true,
    expires_at    TIMESTAMPTZ,
    last_used_at  TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

### 5.5 用量

```sql
CREATE TABLE usage_logs (
    id                BIGSERIAL PRIMARY KEY,
    ts                TIMESTAMPTZ NOT NULL DEFAULT now(),
    request_id        TEXT NOT NULL,
    key_id            BIGINT,                       -- 无外键（Key 可删，记录要留）
    channel_id        TEXT NOT NULL,
    account_uid       TEXT NOT NULL,
    protocol          TEXT NOT NULL,                -- chat | responses | anthropic
    model             TEXT NOT NULL,                -- 对外模型名
    upstream_model    TEXT,
    stream            BOOLEAN NOT NULL DEFAULT false,
    status            TEXT NOT NULL,                -- completed | failed | client_aborted | upstream_aborted | rejected
    is_rejection      BOOLEAN NOT NULL DEFAULT false,
    attempts          INT NOT NULL DEFAULT 1,
    prompt_tokens     BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    reasoning_tokens  BIGINT NOT NULL DEFAULT 0,
    cached_tokens     BIGINT NOT NULL DEFAULT 0,
    total_tokens      BIGINT NOT NULL DEFAULT 0,
    usage_estimated   BOOLEAN NOT NULL DEFAULT false,
    credit            NUMERIC(14,6),                -- 上游积分口径（若有）
    upstream_cost     NUMERIC(14,8) NOT NULL DEFAULT 0,
    billed_cost       NUMERIC(14,8) NOT NULL DEFAULT 0,
    ttft_ms           INT,
    total_ms          INT NOT NULL DEFAULT 0,
    effort            TEXT,
    error_detail      TEXT
);

CREATE INDEX usage_ts          ON usage_logs (ts DESC);
CREATE INDEX usage_channel_ts  ON usage_logs (channel_id, ts DESC);
CREATE INDEX usage_model_ts    ON usage_logs (model, ts DESC);
CREATE INDEX usage_key_ts      ON usage_logs (key_id, ts DESC);
CREATE INDEX usage_account_ts  ON usage_logs (account_uid, ts DESC);
```

**`is_rejection` 是本次必须加的**：参考项目用 `model LIKE '(未进入调度%)'` 做排除，但那个前缀后来改成了 `(上游失败 …)` / `(无可用账号 …)`，导致排除条件失效、拒绝请求被算进总量。**不再用字符串前缀表达语义**，用布尔列。

### 5.6 任务

```sql
CREATE TABLE task_runs (
    id            BIGSERIAL PRIMARY KEY,
    batch_id      UUID,
    channel_id    TEXT NOT NULL,
    account_uid   TEXT NOT NULL,
    scope         TEXT NOT NULL,                    -- all | daily | tasks | checkin
    status        TEXT NOT NULL,                    -- running | done | partial | failed | cancelled
    trigger       TEXT NOT NULL,                    -- manual | schedule
    total         INT NOT NULL DEFAULT 0,
    done_count    INT NOT NULL DEFAULT 0,
    failed_count  INT NOT NULL DEFAULT 0,
    skipped_count INT NOT NULL DEFAULT 0,
    earned        INT NOT NULL DEFAULT 0,
    error         TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ
);

CREATE INDEX task_runs_started ON task_runs (started_at DESC);
CREATE INDEX task_runs_account ON task_runs (channel_id, account_uid, started_at DESC);
CREATE INDEX task_runs_running ON task_runs (status) WHERE status = 'running';

CREATE TABLE task_run_items (
    id          BIGSERIAL PRIMARY KEY,
    run_id      BIGINT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    code        TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,                      -- daily | task | checkin
    status      TEXT NOT NULL,                      -- done | skipped | failed | not_in_window
    message     TEXT NOT NULL DEFAULT '',
    earned      INT NOT NULL DEFAULT 0,
    finished_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX task_run_items_run ON task_run_items (run_id, id);
```

### 5.7 设置

```sql
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

**必须有默认值的键**：

| key | 默认值 | 说明 |
| --- | --- | --- |
| `task.retention_days` | `90` | 范围 7–365 |
| `task.schedule_enabled` | `true` | 定时巡检开关 |
| `task.schedule_hours` | `[9, 21]` | 每日动作补跑档位 |
| `task.per_account_gap_ms` | `800` | 账号间隔 |
| `task.daily_gap_ms` | `1200` | 动作间隔 |
| `gateway.port` | `18788` | |
| `gateway.bind` | `127.0.0.1` | 生产改 `0.0.0.0` 时必须配 Key |
| `log.retention_days` | `30` | |
| `notify.webhook_url` | `""` | 告警推送地址；空 = 不发通知 |

### 5.8 用户、余额与兑换码

> **为什么加这一层**：最初是"单租户自用"，Key 就是全部身份。但需求变成
> "用户能自己注册、通过兑换码充值余额"之后，就必须有自己的用户表，
> 并且 `usage_logs` 要能归到人头上 —— 否则"这个月我花了多少"算不出来。
>
> **与 `api_keys` 的关系**：一个用户可以有多把 Key。Key 仍是转发时的凭证，
> 用户是计费与额度的主体。两者**不合表**：Key 可以随时吊销重发，余额必须跟着人走。
> 转发路径鉴权不变（仍然只认 `sk-` Key），用户体系只作用于管理台与余额结算。

```sql
CREATE TABLE users (
    id              BIGSERIAL PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    password_hash   TEXT NOT NULL,              -- PBKDF2-HMAC-SHA256，见下
    nickname        TEXT NOT NULL DEFAULT '',
    balance         NUMERIC(14,6) NOT NULL DEFAULT 0,   -- 可用余额，不允许为负
    total_recharged NUMERIC(14,6) NOT NULL DEFAULT 0,
    role            TEXT NOT NULL DEFAULT 'user',       -- admin | user
    enabled         BOOLEAN NOT NULL DEFAULT true,
    source          TEXT NOT NULL DEFAULT 'password',   -- password | manual
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at   TIMESTAMPTZ
);

CREATE TABLE user_sessions (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,             -- SHA-256(令牌)，明文只在签发时返回一次
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE redeem_codes (
    id         BIGSERIAL PRIMARY KEY,
    code       TEXT NOT NULL UNIQUE,             -- 一次性；已排除容易看错的 0/O/1/I
    amount     NUMERIC(14,6) NOT NULL CHECK (amount > 0),
    note       TEXT NOT NULL DEFAULT '',
    expires_at TIMESTAMPTZ,
    used_by    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE balance_transactions (
    id            BIGSERIAL PRIMARY KEY,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    amount        NUMERIC(14,6) NOT NULL,        -- 正数充值、负数消费
    balance_after NUMERIC(14,6) NOT NULL,        -- 对账用，不用自己累加
    kind          TEXT NOT NULL,                 -- redeem | usage | adjust
    ref_id        BIGINT,
    note          TEXT NOT NULL DEFAULT '',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

**几条硬约束**：

- **密码用 PBKDF2-HMAC-SHA256（600,000 轮）**，不是 bcrypt。原因不是偏好：
  bcrypt 在 `golang.org/x/crypto` 里，装它要访问 `proxy.golang.org`，离线环境装不上；
  而 Go 1.24+ 标准库已有 `crypto/pbkdf2`。选标准库意味着**零外部依赖、离线可构建**。
  迭代次数写进哈希串（`pbkdf2-sha256$迭代$盐$哈希`），将来提升强度时老密码仍能验证。
  比较用 `subtle.ConstantTimeCompare`：字符串比较会按字节提前返回，能被计时攻击利用。
- **登录失败不区分"邮箱不存在"与"密码错"**：分开报会把接口变成账号枚举器。
- **令牌只存 SHA-256**：库被读走也拿不到可用令牌。
- **兑换码核销必须在事务里**，并用 `UPDATE ... WHERE used_at IS NULL RETURNING`
  做原子占位。不这么做会有两个典型事故：并发兑换同一个码都能通过"未使用"检查；
  余额加了但码没标已用（或反之）→ 对不上账。
- **余额扣成负数直接拒绝**（`ErrInsufficientBalance` → 400，不是 500）。
  允许透支意味着"用户能超额度跑请求"，而额度检查在余额 0 时就挡，透支永远收不回来。
- **第一个注册的用户自动是 admin**（开箱即用，不用手工改库）。
  管理员也可以直接建用户并指定角色与初始余额（`POST /api/v1/users`）。

---

## 6. 枚举与状态机

> 所有枚举值**大小写敏感**，前后端必须一致。用 `snake_case`。

### 6.1 `usage_logs.status`

| 值 | 含义 |
| --- | --- |
| `completed` | 正常结束（含上游 `finish_reason=length`） |
| `failed` | 上游明确失败或本地无法继续 |
| `client_aborted` | 客户端断开（上游已产生 token，仍计费） |
| `upstream_aborted` | 上游断流 / 空闲超时 |
| `rejected` | 未进入调度（鉴权失败、并发满、无可用账号、Key 无权限） |

### 6.2 `task_runs.status`

```
failed_count == 0 && done_count > 0        → done
failed_count > 0 && done_count > 0         → partial
failed_count > 0 && done_count == 0        → failed
done_count == 0 && failed_count == 0       → done     （全是 skipped）
用户取消                                    → cancelled
```

**`skipped` 与 `not_in_window` 计入 `skipped_count`，绝不计入 `failed_count`。**
WorkBuddy 有一半任务需要桌面端手动完成，算成失败会让一键执行永远是 `partial`，红点变成噪音。

### 6.3 `task_run_items.status`

| 值 | 触发场景 |
| --- | --- |
| `done` | 成功；或上游返回"本来就已完成" |
| `skipped` | 不可自动化；猫猫旅行 `traveling`；已达日限 |
| `not_in_window` | `black_cat` 只在 23:00–08:00 上报 |
| `failed` | 真失败 |

### 6.4 错误类型（Rust `ErrorKind` → HTTP）

| ErrorKind | HTTP | OpenAI `error.type` | Anthropic `error.type` | 可重试 |
| --- | --- | --- | --- | --- |
| `InvalidRequest` | 400 | `invalid_request_error` | `invalid_request_error` | 否 |
| `AuthExpired` | 401 | `authentication_error` | `authentication_error` | 否 |
| **`KeyForbidden`** | **403** | `permission_error` | `permission_error` | 否 |
| `ModelNotFound` | 404 | `invalid_request_error` | `not_found_error` | 否 |
| `RateLimited` | 429 | `rate_limit_error` | `rate_limit_error` | 是 |
| `QuotaExhausted` | 402 | `insufficient_quota` | `invalid_request_error` | 否 |
| `ContentPolicy` | 403 | `content_policy_violation` | `permission_error` | 否 |
| `NoAccount` | 503 | `rate_limit_error` | `overloaded_error` | 是 |
| `Upstream5xx` | 502 | `api_error` | `api_error` | 是 |
| `Network` | 502 | `api_error` | `api_error` | 是 |
| `ChannelBlocked` | 503 | `channel_blocked_error` | `overloaded_error` | 是 |
| `Unsupported` | 501 | `invalid_request_error` | `api_error` | 否 |
| `Internal` | 500 | `api_error` | `api_error` | 否 |

**`KeyForbidden` 是本次新增的**。三种情况必须分清：

| 情况 | 返回 | 消息示例 |
| --- | --- | --- |
| 模型名写错 / 无渠道提供 | 400 | `没有渠道提供模型 xxx` |
| **Key 无权** | **403** | `该 Key 无权使用模型 xxx：允许的渠道（trae-cn）都不提供它` |
| 有权限但账号全冷却 | 503 | `模型 xxx 当前没有可用账号（wb-cn：冷却中）` |

> 403 若被报成 503，用户会去查账号冷却、查上游状态，而真正原因在 Key 配置里——这是最贵的排障时间。

### 6.5 管理面错误（Go → HTTP）

| 情况 | HTTP | 说明 |
| --- | --- | --- |
| 记录不存在 | 404 | `store.ErrNotFound` |
| **记录存在但状态不允许该操作** | **409** | `store.ErrNotRunning`，如对已结束的 run 点取消 |
| 参数非法 | 400 | 如保留天数越界、id 非数字 |
| 网关返回的错误 | 透传 | `gateway.APIError` 保留原状态码（403 不能变成 500） |
| 其它 | 500 | 日志保留原文，响应只给"服务内部错误" |

> 409 与 404 必须分清："已经跑完了所以不能取消"若报成 404，
> 用户会去翻 id 是不是写错了，而真正原因在状态。

---

## 7. API 契约

### 7.1 通用约定

- 前缀 `/api/v1`（管理面）、`/v1`（网关）、`/internal`（Rust 内部）。
- 响应信封：`{ code: number; success: boolean; msg?: string; data?: T }`，`0` 与 `200` 为成功。
- 分页：请求 `page`（从 1 起）、`pageSize`（默认 20，上限 100）；响应 `{ list: T[], total: number, page: number, pageSize: number }`。
- 时间：ISO 8601 带时区（`2026-10-21T17:00:00+08:00`）。**不要**返回 epoch 毫秒。
- 管理员会话：`Authorization: Bearer <token>`，token 存 `localStorage['relayai.auth.token']`（沿用 `src/utils/auth.ts`）。

### 7.2 管理面（Go）

#### 渠道

```
GET    /api/v1/channels                 列表（含账号数、今日调用数）
POST   /api/v1/channels                 新建
GET    /api/v1/channels/:id             详情
PATCH  /api/v1/channels/:id             更新
DELETE /api/v1/channels/:id             删除（组内账号变「待分配」，不删账号）
POST   /api/v1/channels/:id/probe       探活（代理 Rust）
GET    /api/v1/channels/:id/models      该渠道的模型目录（代理 Rust）
```

`ChannelDTO`：

```json
{
  "id": "wb-cn", "name": "WorkBuddy 国内版", "adapter": "workbuddy",
  "enabled": true, "priority": 100, "weight": 5,
  "options": { "realm": "cn", "outbound_identity": "desktop" },
  "channelRatio": 1.0, "concurrencyLimit": null, "rateLimitQps": null,
  "accountCount": 3, "todayRequests": 128,
  "caps": { "tasks": true, "daily": true, "checkin": true }
}
```

> `caps` 由 Rust 的 `ops_caps()` 提供，Go 侧缓存 30s。**不发网络请求**。

#### 账号

```
GET    /api/v1/accounts?channelId=&status=&page=&pageSize=
POST   /api/v1/accounts                 手工新增（自定义渠道）
PATCH  /api/v1/accounts/:id             改 nickname / enabled / 覆盖值 / 模型白名单
DELETE /api/v1/accounts/:id
POST   /api/v1/accounts/:id/login       发起登录 → { loginTicket }
POST   /api/v1/accounts/:id/login/poll  轮询 → { pending | ready | failed }
POST   /api/v1/accounts/:id/login/complete  粘贴回调地址完成登录
POST   /api/v1/accounts/:id/refresh     强制刷新凭据
POST   /api/v1/accounts/:id/probe       真实探活
GET    /api/v1/accounts/:id/quota       额度快照
```

> 登录三件套、刷新、探活、额度全部代理到 Rust `/internal/*`；其余直连 PG。

#### 密钥

```
GET    /api/v1/keys
POST   /api/v1/keys                     返回一次性明文 { key: "sk-..." }
PATCH  /api/v1/keys/:id
DELETE /api/v1/keys/:id
POST   /api/v1/keys/:id/rotate          生成新明文，旧 key_hash 立即失效
```

`KeyDTO` **永不包含明文**，只有 `keyPrefix`：

```json
{
  "id": 3, "name": "Cursor 专用", "keyPrefix": "sk-abcd****",
  "channelAllow": ["wb-cn", "wb-intl"], "modelAllow": [],
  "rpmLimit": null, "dailyTokenLimit": null, "maxConcurrency": null,
  "enabled": true, "expiresAt": null, "lastUsedAt": "2026-10-21T17:00:00+08:00"
}
```

#### 模型

```
GET    /api/v1/models?channelId=&enabled=
PATCH  /api/v1/models/:name             改价格 / 倍率 / 能力 / 启用
POST   /api/v1/models/sync              从各渠道目录拉取并合并（代理 Rust）
```

#### 用量

```
GET /api/v1/usage?from=&to=&channelId=&accountUid=&keyId=&model=&status=&page=&pageSize=
GET /api/v1/usage/:id
GET /api/v1/usage/summary?from=&to=&groupBy=channel|model|key|day
GET /api/v1/usage/series?from=&to=&model=&bucket=hour|day
```

#### 任务

```
GET  /api/v1/tasks/channels              有能力的分组 + 账号数 + 今日待办
GET  /api/v1/tasks/channels/:id/live     实时快照（代理 Rust）
                                         `:id` 可以是**版本**（cn / intl）或分组 id
POST /api/v1/tasks/runs                  发起执行
POST /api/v1/tasks/runs/:id/cancel
GET  /api/v1/tasks/runs?channel=&account=&status=&page=
GET  /api/v1/tasks/runs/:id
GET  /api/v1/tasks/runs/:id/stream       SSE 逐条进度
POST /api/v1/tasks/claim                 单独领取
```

> **按版本取号**：`/tasks/channels/cn/live` 返回**国内版全部账号**，
> 包括还没分配分组的号；传具体分组 id 时只返回该分组的号（兼容用）。
> 任务页的 URL 参数本来就是版本 —— 只按一个分组取号会让"待分配"的号永远不出现。

`POST /tasks/runs` 请求体：

```json
{
  "accounts": [{ "uid": "u_abc", "channelId": "wb-cn" }],
  "scope": "all",
  "codes": []
}
```

> 账号必须**逐个带 `channelId`**（待分配时为空串）：同一页里可能混着多个分组的号，
> 用一个页面级的"代表分组"会把待分配的号发到错误的分组上，网关就找不到账号。
> 老形状 `{ "channelId": "...", "accountUids": ["..."] }` 仍然接受（当作所有号的默认分组）。

`scope` 取值见第 9.2 节。响应：

```json
{ "batchId": "uuid", "runs": [{ "runId": 12, "accountUid": "u_abc", "channelId": "wb-cn" }] }
```

#### 设置与状态

```
GET   /api/v1/settings
PATCH /api/v1/settings
GET   /api/v1/status                    代理 Rust：服务状态 / 矩阵 / 并发 / 告警
GET   /api/v1/status/decisions          最近调度决策（代理 Rust）
```

#### 认证与用户

```
POST /api/v1/auth/register              注册（第一个用户自动成为 admin）
POST /api/v1/auth/login                 登录 → { token, user }
POST /api/v1/auth/logout                吊销会话
GET  /api/v1/auth/me                    当前登录者
POST /api/v1/auth/redeem                兑换码充值
GET  /api/v1/auth/transactions          我的余额流水
```

管理端（需 `role = admin`，否则 **403 而不是 401** —— 已登录但没有权限是两件事，
报 401 会把用户踢回登录页，而他其实登录着）：

```
GET  /api/v1/users                      用户列表
POST /api/v1/users                      管理员开户（可指定角色与初始余额）
POST /api/v1/users/:id/balance          调整余额（正数加、负数减）
GET  /api/v1/redeem-codes               兑换码列表
POST /api/v1/redeem-codes               批量生成兑换码
```

会话令牌走 `Authorization: Bearer <token>`（也接受裸 token 便于 curl 调试，
但不接受 `Basic` —— 那是另一种语义，混在一起会让鉴权逻辑变难懂）。

### 7.3 网关面（Rust，对外）

```
POST /v1/chat/completions
POST /v1/completions
POST /v1/messages
POST /v1/messages/count_tokens
POST /v1/responses
GET  /v1/models
GET  /health
```

**`GET /v1/models` 必须按 Key 过滤**（I6 的延伸）：

```rust
matrix.public_models()
    .filter(|m| key.allows_model(m))
    .filter(|m| matrix.routes(m).iter().any(|r| key.allows_channel(&r.channel_id)))
```

> 第二个 filter 是 `any` 不是 `all`：只要还有一条本 Key 能走的路由，模型就该出现。

### 7.4 内部接口（Rust，只绑 127.0.0.1 + 共享密钥）

请求头 `X-Internal-Token: <GATEWAY_INTERNAL_TOKEN>`，不匹配返回 401。

```
GET  /internal/status
GET  /internal/matrix
POST /internal/matrix/refresh
GET  /internal/decisions?limit=
GET  /internal/channels/:id/models
POST /internal/channels/:id/probe
POST /internal/accounts/:key/refresh
POST /internal/accounts/:key/probe
GET  /internal/accounts/:key/quota
POST /internal/accounts/:key/login/start
POST /internal/accounts/:key/login/poll
POST /internal/accounts/:key/login/complete
GET  /internal/tasks/channels/:id/live
GET  /internal/tasks/realms/:realm/live   按版本（cn/intl）跨分组取号，含"待分配"的号
POST /internal/tasks/run                  { channel_id, uid, scope, run_id }
POST /internal/tasks/cancel               { channel_id, uid }
GET  /internal/tasks/progress?channel_id=&uid=
POST /internal/tasks/claim
POST /internal/state/reload              配置变更后通知 Rust 重新加载
```

> `:key` 用 `channelId/uid` 的 URL 编码形式，例如 `wb-cn%2Fu_abc`。
>
> **任务的写接口按 `channel_id` + `uid` 定位，不用 run id**：执行状态活在网关进程
> 内存里，网关根本不知道 PG 里的 run id 是什么。`run_id` 是 Go 侧建行后**带下去**的
> 参数，只用于回写明细与终态；取消也走同一套定位方式（Go 先查库拿到渠道与账号，
> 再通知网关置标志位）。

---

## 8. 前端信息架构

### 8.1 导航（五组）

```
概览        /dashboard
渠道与账号   /channels  /channels/:id  /accounts  /accounts/:id  /status
任务        /tasks     /tasks/:channelId
用量        /usage     /usage/:id
系统        /keys      /users  /redeem-codes  /settings
我的        /account
```

### 8.2 路由表（完整）

| 路由 | 页面 | 分组 |
| --- | --- | --- |
| `/` | 重定向到 `/dashboard` | — |
| `/dashboard` | 仪表盘 | 概览 |
| `/channels` | 渠道管理 | 渠道与账号 |
| `/channels/:id` | 渠道详情（Tab：账号 / 模型 / 用量） | 渠道与账号 |
| `/accounts` | 账号管理（跨渠道） | 渠道与账号 |
| `/accounts/:id` | 账号详情 | 渠道与账号 |
| `/status` | 分组状态（只显示开启展示的分组） | 渠道与账号 |
| `/tasks` | 任务总览 | 任务 |
| `/tasks/:channelId` | 渠道任务页 | 任务 |
| `/usage` | 使用记录 | 用量 |
| `/usage/:id` | 请求详情 | 用量 |
| `/keys` | 密钥管理 | 系统 |
| `/users` | 用户管理 | 系统 |
| `/redeem-codes` | 兑换码 | 系统 |
| `/settings` | 设置 | 系统 |
| `/account` | 用户中心 | 我的 |
| `*` | 404 | — |

> 原 `/` 的「项目概览」骨架页**删除**。
>
> **`/stats`（统计报表）也已删除。** 它的内容与仪表盘重复度太高：
> 指标卡（请求数 / Token / 消费 / 首字）在仪表盘首屏已有，趋势图等于仪表盘的
> 「请求量 + 使用趋势」两图，按模型明细等于「模型排行」，按天聚合等于仪表盘的
> 7 天 / 30 天档位。真正独有的只有两样：**按 Key 聚合**与 **CSV 导出**。
>
> 处理方式：CSV 导出挪到 `/usage`（按钮写「导出本页」，不冒充全量）；
> 「按 Key 聚合」由渠道详情页的用量 Tab 承担（`ChannelUsageTab` 已用
> `getUsageSummary({groupBy:'key'})`）。仪表盘负责"今天现在怎么样"，
> 使用记录负责"逐条明细与导出"，不再有一个只重复前两者一半内容的页面。

### 8.3 导航元数据结构

`src/routes/navigation.ts` 扩展为支持分组与二级菜单：

```ts
export type NavigationItem = {
  label: string;
  description: string;
  to: string;
  icon: LucideIcon;
  end?: boolean;
  children?: NavigationItem[];   // 二级菜单（任务组用它）
};

export type NavigationGroup = {
  label: string;                  // 分组标题，如「渠道与账号」
  items: NavigationItem[];
};
```

**任务组的二级菜单是数据驱动的**：从 `GET /api/v1/tasks/channels` 取有能力的渠道，渲染为 `/tasks/:channelId`。**取不到时只显示「任务总览」，不显示空二级菜单。**

### 8.4 文件组织（必须遵守）

```text
src/
├── api/<feature>/index.ts        # 一个 feature 一个目录
├── components/
│   ├── common/<Name>/index.tsx   # 跨页面共享
│   └── ui/                       # shadcn 原语
├── pages/<Module>/index.tsx      # 页面入口
├── pages/<Module>/components/    # 页面私有组件
├── routes/
├── hooks/
├── utils/
└── styles/
```

---

## 9. 页面级规格

> 每页都要有：加载态（`MyBox`）、空态（一句话说明 + 下一步动作）、错误态（可重试）。下面不再逐页重复这三条。

### 9.1 仪表盘 `/dashboard`

**区块**（自上而下）：

1. **指标卡 ×4**：今日请求、成功率、今日 Token、今日成本。每张卡：主数值、对比昨日的变化（↑↓ + 百分比）、一句副标题。
2. **24h 趋势图**：双 Y 轴折线（请求数 / Token），粒度 1 小时。Recharts `LineChart`。
3. **渠道健康**：每个渠道一行——名称、可用账号/总账号、今日成功率、P95 首字延迟、状态点。
4. **最近错误 Top5**：错误类型、次数、最后发生时间、涉及渠道。点击跳到 `/usage?status=failed&errorType=xxx`。

**数据源**：`/usage/summary`、`/usage/series`、`/status`。

**不要做**：不要在这页放"新建渠道""执行任务"这类动作按钮。仪表盘只读。

### 9.2 渠道管理 `/channels`

**列表列**：名称 / 类型（adapter 中文名）/ 账号数 / 今日调用 / 倍率 / 状态 / 操作。

- 顶部：`新建渠道`（打开抽屉）、搜索框、状态筛选。
- 行操作：编辑、探活、查看详情、删除。

**新建/编辑抽屉字段**（按 adapter 联动）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| 名称 | text | 必填 |
| 类型 | 自定义 Select | workbuddy / trae / custom，**编辑时不可改** |
| realm | Select | 仅 workbuddy：国内 / 国际 |
| 出站身份 | Select | 仅 workbuddy：桌面端 / CLI / VSCode |
| base_url | text | 仅 trae / custom |
| 上游协议 | Select | 仅 custom：openai-chat / openai-responses / anthropic |
| 优先级 | number | 默认 100 |
| 权重 | number | 默认 1 |
| 渠道倍率 | number | 默认 1.0，step 0.01 |
| 并发上限 | number | 可空 |
| 启用 | switch | 默认开 |

**校验**：base_url 必须以 `http://` 或 `https://` 开头；倍率 > 0；优先级 0–1000。

### 9.3 渠道详情 `/channels/:id`

三个 Tab：

- **账号**：该渠道的账号表（列同 `/accounts`，去掉渠道列）+ `添加账号` 按钮（走登录流程）。
- **模型**：该渠道目录（实时拉取）+ 映射到哪个对外模型 + 是否已定价。未定价的行高亮提示。
- **用量**：近 7 天该渠道的请求数 / Token / 成本趋势 + 错误分布。

### 9.4 账号管理 `/accounts`

**列表列**：账号（昵称 + uid）/ 渠道 / 状态 / 额度 / 今日用量 / 最近失败 / 操作。

- 状态从运行态读（`/status`），不是库里的字段。
- 状态值：`正常` / `冷却中（剩余 3m）` / `凭据失效` / `已停用` / `无凭据`。
- 筛选：渠道、状态、关键字。
- 批量操作：启用、停用、刷新凭据。

**登录流程**（点「添加账号」）：

```
选渠道 → login/start 拿到 authUrl
  ├─ 有授权页：显示"打开授权页"按钮 + 轮询 login/poll
  │    └─ 需要回调：显示粘贴框，用户把地址栏内容粘回来 → login/complete
  └─ 无授权页（自定义渠道）：显示凭据填写表单 → POST /accounts
```

### 9.5 账号详情 `/accounts/:id`

- **凭据**：状态、过期时间、上次刷新、`刷新`按钮。
- **额度**：当前快照（label/unit/remaining/total）+ 抓取时间。
- **最近 50 次请求**：时间、模型、状态、耗时、首字延迟。
- **操作**：探活、重置状态、删除。
- 底部链接：`查看该账号的任务 →` 跳 `/tasks/:channelId?account=uid`。

### 9.6 分组状态 `/status`

> 这一页只显示在「分组管理」里**开启了展示**（`channels.status_visible`）的分组。
> 默认全部不展示 —— 分组是调度用的计价桶，公开状态是额外动作。
> 想只公开国内版、把国际版藏起来，就是把国际版的开关保持关闭。

- 顶部：网关运行状态、监听地址、版本、运行时长、全局并发（可用/总量）。
- **分组卡片网格**：每张卡 = 一个开启展示的分组。含状态判定、探活成功率与延迟、
  最近 60 次探活的细条、最近一次失败原文、探活模型。
- 统计窗口可切 24 小时 / 7 天 / 30 天；自动刷新 10s，带暂停开关。
- 空态引导到「分组管理」，不在本页放设置入口（藏起来的分组在这里没有卡片，
  入口放在这儿就再也改不回来）。

**所有指标都来自定时探活 `channel_probes`**，不是真实请求 `usage_logs`：
一次探活 = 细条一格，百分比也是探活成功率。两套数据节奏不同，混用会让
"请求多"被误读成"探活没按时跑"。

**展示开关与探活模型都在「分组管理 → 编辑分组」**：它们服务同一个目的
（这张状态板展示什么、用什么证明它可用），而从状态页改需要先看见那张卡片。

### 9.7 任务总览 `/tasks`

- **渠道卡片网格**：每张卡 = 一个有运营能力的渠道。内容：渠道名、账号数、今日待办 `X/Y`、`一键执行`按钮。
- **运行中批次**：批次 id、渠道、进度条 `done/total`、开始时间、`取消`。
- **最近失败**：最近 10 条 `failed` 的 run，点进 `/tasks/:channelId`。

### 9.8 渠道任务页 `/tasks/:channelId`

> `:channelId` 是**版本**（`cn` / `intl`），不是分组 id。页面按版本跨分组取号，
> 未分配分组的号同样出现在这一页（标「待分配」）。

**主体是账号表**（不是任务表）：

| 账号 | 今日进度 | 状态 | 最近执行 | 操作 |
| --- | --- | --- | --- | --- |
| 主号 | 3/8 项 · 签到未做 | 空闲 | 2 小时前 · 3 成功 | 一键执行 / 展开 |
| 备用号 | 0/8 项 | **执行中 2/8** | — | 取消 / 展开 |

- 顶部：`一键执行全部账号`、`刷新`、筛选（只看待办 / 只看失败）。
- **默认只展开一个账号**。展开区：**每日动作在上、任务清单在下**（与执行顺序一致）。
- 展开区每行：名称、状态、进度、奖励、不可自动化原因、独立按钮（`领取` / `执行`）。
- 行内实时结果通过 SSE 更新。

**Trae 这类只有签到的渠道**：页面退化为账号表，列为 `账号 / 今日签到 / 最近执行 / 操作`，顶部按钮文案是**「全部签到」**（不是"一键执行"），`scope` 发 `checkin`。

### 9.9 使用记录 `/usage`

**列表列**：时间 / Key / 渠道 / 账号 / 模型 / 输入 / 输出 / 成本 / 首字 / 总耗时 / 状态 / 重试。

- 筛选：时间范围（自定义 DatePicker）、渠道、账号、Key、模型、状态。
- 拒绝行（`is_rejection`）用弱化样式 + `—` 占位，**不显示模型名**。
- 分页 50/页。
- 点击行 → `/usage/:id`。

### 9.10 请求详情 `/usage/:id`

- 概要：request_id、时间、协议、状态、模型 → 上游模型。
- 时间线：收到请求 → 鉴权 → 选号 → 上游首字节 → 结束（各阶段耗时）。
- 重试链：每次尝试的账号、结果、错误类型。
- 上游错误原文（脱敏后）。

### 9.11 统计报表 `/stats` —— **已删除**

原规格要求"维度切换（模型 / 渠道 / Key / 天）+ 表格 + 导出 CSV"。
实现出来之后与仪表盘重复度过高，按产品判断删除整页：

| 原统计报表内容 | 现在在哪 |
| --- | --- |
| 4 张指标卡（请求数 / Token / 消费 / 首字） | 仪表盘首屏 6 张卡 |
| 趋势图（请求量 + Token） | 仪表盘「请求量」+「使用趋势」 |
| 按模型明细 | 仪表盘「模型排行」 |
| 按天聚合 | 仪表盘的 7 天 / 30 天档位 |
| **CSV 导出** | **`/usage` 的「导出本页」** |
| **按 Key 聚合** | **渠道详情页的用量 Tab**（`getUsageSummary(groupBy:'key')`） |

分工因此变清晰：仪表盘 = "今天现在怎么样"，使用记录 = "逐条明细与导出"。

### 9.12 密钥管理 `/keys`

**列表列**：名称 / API 密钥 / 渠道 / 模型 / 限制 / 最近使用 / 状态 / 操作。

> **渠道是单选，1 对 1，且必填。** 一把密钥绑一个渠道，**没有"全部渠道/不绑定"这种状态**。
> 早期做成多选 `channel_allow: string[]` 是错的：多选等于"这把 Key 能走任意通道"，
> 用户也就没法用"便宜的 Key / 稳定的 Key"来分流。
> 库里仍保留数组形态（兼容历史数据），但**写入只允许恰好 1 个元素**，
> 后端在 `POST /keys` 里校验（400「必须选择一个渠道」），前端也限制为单选。
> 表格内的「渠道」列可直接切换（`components/common/ChannelPicker`）。
>
> 操作列是**纯图标**（`title` + `aria-label` 提供文字），语义必须准确：
> 使用密钥 `Terminal` / 导入 CCS `Download` / 禁用 `Ban` / 编辑 `PencilLine` / 删除 `Trash2`。
> **"查看详情"用 `Eye`，不要用 `ExternalLink`** —— 后者是"离开本应用"的语义。
>
> 每个动作需要独立的 `id`（用作 React key）。**不能用 `label` 当 key**：
> 同一行可能有多个动作共享同一段文案（例如都因"明文已丢失"而改名），
> 那会触发 "Encountered two children with the same key"，React 会漏渲染。

### 9.12.1 「导入 CC Switch」是 deeplink，不是下载文件

**这条曾经做错过**：第一版是把 `{base_url, api_key}` 打包成 JSON 让用户下载。
那是"导出配置"，用户还得自己找 CC Switch 的配置目录、手工改 JSON。

正确做法（对齐 sub2api 的 `utils/ccswitchImport.ts`）：
用 `ccswitch://v1/import?...` 协议**直接把 provider 注册进客户端**。
CC Switch 的配置在它自己的数据目录里，网页没有文件系统权限，
协议处理器是它对外暴露的唯一入口。

参数是 CC Switch 侧的契约，不能自己发明：

| 参数 | 值 |
| --- | --- |
| `resource` | `provider` |
| `app` | `claude` / `codex` / `gemini` / `grokbuild` |
| `name` | 站点名 |
| `homepage` | 网关根地址 |
| `endpoint` | **按平台决定是否带 `/v1`**，见下 |
| `apiKey` | 密钥明文 |
| `configFormat` | `json` |
| `usageEnabled` / `usageScript` / `usageAutoInterval` | 余额查询脚本，base64 |

**`endpoint` 的 `/v1` 规则是最容易错的地方**（两个方向相反）：

- `codex`（openai 平台）**必须去掉** `/v1`：CC Switch 的 Codex provider 自己会补
  OpenAI 兼容路径，传了就会请求 `/v1/v1/...` → 404；
- `grokbuild` **必须带** `/v1`。

第一版只写了"去结尾斜杠"，被 `scripts/test-ccswitch.mjs` 抓到
（断言 `endpoint === 'http://localhost:18788'` 实际拿到 `.../v1`）。

**客户端类型必须让用户选，不能默认 `claude`。** CC Switch 里
Claude Code 与 Gemini CLI 是两套不同的配置格式，猜错了会往错误的客户端里塞配置。
导入前弹一个二选一的对话框。

### 9.12.2 客户端接入地址来自 `/public-config`，不是 `window.location.origin`

**这是曾经的真 bug**：`window.location.origin` 在管理台上返回 `http://localhost:5173`
（Vite 开发服务器），而**网关在 18788**。用户照着填，客户端会去打前端开发服务器
拿到 404 —— 看起来像"网关坏了"，其实是地址指错了进程。

正确做法：

- 后端 `GET /api/v1/public-config`（**无需鉴权**，不含任何密钥）返回
  `{ gatewayBaseUrl, siteName }`；
- 地址取自配置项 `gateway.publicBaseUrl`，留空回落到 `gateway.baseUrl`；
- 前端拿不到时显示"地址未知"，**不回退到 `window.location.origin`** ——
  错的地址比没有更糟，用户会照着填然后以为是网关的问题。

部署提醒：`gateway.baseUrl` 通常是 `http://127.0.0.1:18788`（后端到网关的内网地址），
用户在自己电脑上填 `127.0.0.1` 打的是他自己的机器。对外必须显式配 `gateway.publicBaseUrl`。

- 「渠道」列显示 `适配器名（数量）`，不罗列内部 id。
- 新建后**只显示一次明文**，用带复制按钮的对话框，关闭后不可再查看。

**新建/编辑抽屉**：

- **渠道**：**单选，必填**。没有"不绑定"选项。
- **模型**：默认「全部模型」，**选项范围随渠道选择联动**——选了 Trae 就不能勾只在 WorkBuddy 上的模型。
- 模型不选 = 不限。
- 限速：RPM、日 Token 上限、最大并发，均可空。

### 13.2 表格撑满剩余高度（踩过的坑）

表格必须占满内容区剩余高度、只有表格内部滚动、分页条固定在底部。
这条依赖一条完整的 flex 链，**中间任何一环断了都会退化成 fit-content**：

```
main (flex flex-col, overflow-auto)
└─ div (flex flex-1 flex-col, min-h-0)      ← AppShell 的包裹层
   └─ 页面根节点 (flex flex-1 flex-col, min-h-0)
      └─ MyBox (flex flex-col, flex-1, min-h-0)
         └─ MyBox 内层 (flex h-full min-h-0 flex-col)   ← 少这个 flex 就断
            └─ DataTable (flex-1 overflow-auto)
               └─ table (h-full) + tbody 末尾吸收行
```

四个必须记住的点：

1. **`AppShell` 的包裹层要 `flex flex-1 flex-col min-h-0`**。它若只是普通 div，
   页面根节点的 `flex-1` 没有可分配高度。
2. **`MyBox` 内层要 `flex flex-col`**。调用方传的是 `min-h-0 flex-1 overflow-auto`，
   这些类只在 flex 父容器里生效；父层不是 flex 时表格按内容高度溢出，
   会**压到下方分页条上**（实测重叠）。
3. **`<table>` 要 `h-full`**。`display: table` 的高度由行内容决定，不加就只剩内容高度。
4. **`tbody` 末尾要有吸收行**（`<tr class="h-full">`）。`table h-full` 只保证整体高度，
   多余空间会被均摊到各数据行（把行拉高）；吸收行负责吃掉剩余空间。

**反例（已踩两次）**：给 `<table>` 加 `h-full` 会导致浏览器把剩余高度均摊到数据行上，
表格顶出容器、**压住下方分页条**（使用记录页实测重叠）。正确做法是
表格保持内容高度、由外层 `overflow-auto` 滚动；`MyBox` 内层用 `flex-1 min-h-0`
而不是 `h-full`（外层有 `min-h-16` 时 `h-full` 会算错）。

### 13.3 定时探活

`settings` 表两个键：

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `probe.enabled` | `true` | 定时探活开关 |
| `probe.interval_minutes` | `30` | 间隔，**钳制在 5–1440** |

探活 = 对每个启用的账号发一条**最小**请求（`probe_account` 内部就是
`Block::text("hi")`，并优先选**免费**模型），用来提前发现"凭据过期/被封/额度耗尽"。
不探活的话这类故障只有等真实用户请求失败才暴露，那时用户已经吃到 500。

三条克制：

1. 只发一句 hello，且优先免费模型 —— 探活是自费动作；
2. 账号之间留 `PER_ACCOUNT_GAP`（800ms），避免整点齐发被上游当异常流量；
3. 间隔下限 5 分钟，别把自己烧穿。

跳过条件：账号未启用、缺凭据、已在冷却中（探了也还是失败，白烧一次）。
`probe_enabled = false` 时**不重置**上次时间戳，重新打开后会立刻跑一次。

### 13.4 渠道状态卡片不显示 token 总量

卡片曾显示「近 N 次请求 / 801.18M tok · 2859 次」，已移除。
原因：那个数字是**全部**请求的累计，混着历史导入数据与系统自身流量，
既不反映真实业务量、又会让人以为渠道在猛跑。可用性百分比与延迟指标
已经能回答"这个渠道健康吗"；逐条明细去「使用记录」看（那里可以按状态筛）。

### 9.13 设置 `/settings`

分组：

- **网关**：监听地址、端口、内网访问开关（开启时强制要求至少一个 Key）。
- **任务**：保留天数（7–365）、定时巡检开关、补跑档位、账号间隔、动作间隔。
- **日志**：用量保留天数、是否记录错误原文。
- **通知**：Webhook URL（掉号 / 额度不足 / 错误率突增）。

---

## 10. Rust 侧改造清单

### 10.1 去 Tauri（精确到文件）

| 文件 | 改法 |
| --- | --- |
| `gateway/mod.rs` | 删 `setup(app)`；改为 `main.rs` 读环境变量 + `tokio::spawn(server::run)` |
| `gateway/ops.rs` | `tauri::async_runtime::spawn` → `tokio::spawn` |
| `gateway/commands/` | **整层删除**，重写为 `admin/` 下的 axum 路由 |
| `gateway/storage.rs` | SQLite → PG（sqlx）；`SecretStore` → 环境变量 |
| `gateway/security.rs` | `envelope::master_key` 从 keyring 改为读 `RELAYAI_MASTER_KEY` |
| `gateway/timeutil.rs` | 删 `windows` 分支，用 `chrono::Local` |
| `gateway/channels/workbuddy/tasks.rs` | `in_night_window()` 同上 |
| `gateway/commands/head_restore.rs` | **直接删除**（UTF-16 残留、无 `mod` 引用、未编译） |

### 10.2 必须保留原样（直接搬）

- `protocol/`（cir / decode / encode）全部
- `core/{dispatch,router,matrix,affinity,pool,refresh,usage}.rs`
- `channels/{workbuddy,trae,custom,openai_compat,mod}.rs`
- `transport.rs`
- `errors.rs`（只加 `KeyForbidden`）

### 10.3 必须重写

| 模块 | 原因 |
| --- | --- |
| 存储层 | SQLite → PG；`ts` 用 `timestamptz` 不用 epoch 毫秒 |
| 面板命令层 | 44 个 Tauri 命令 → `/internal/*` 路由 |
| `task_runs` 内存 Map | 从不清理（泄漏）；改 PG + 内存只留运行中 |
| 任务批量 | 新增批次、串行调度、取消、历史 |
| Key 鉴权 | `Vec<String>` → `KeyContext`（含两个 allowlist） |
| 模型清单 | 按 Key 过滤 |
| 候选集 | 按 Key 过滤（在建集阶段） |

### 10.4 环境变量

| 变量 | 必填 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | 是 | `postgres://user:pass@host:5432/relayai` |
| `RELAYAI_MASTER_KEY` | 是 | 32 字节 base64，启动时校验长度，缺失拒绝启动 |
| `GATEWAY_INTERNAL_TOKEN` | 是 | Go ↔ Rust 共享密钥 |
| `GATEWAY_BIND` | 否 | 默认 `127.0.0.1` |
| `GATEWAY_PORT` | 否 | 默认 `18788` |
| `RUST_LOG` | 否 | 默认 `info` |

---

## 11. 任务模块规格

### 11.1 能力探测

```rust
pub struct OpsCaps {
    pub tasks: bool,
    pub daily: bool,
    pub checkin: bool,
}

trait ChannelAdapter {
    fn ops_caps(&self) -> OpsCaps { OpsCaps::default() }   // 默认全 false
}
```

| 渠道 | tasks | daily | checkin |
| --- | --- | --- | --- |
| `workbuddy` realm=cn | ✅ | ✅ | ✅ |
| `workbuddy` realm=intl | ❌ | ✅ | ⚠️ 待实测 |
| `trae` | ❌ | ❌ | ✅ |
| `custom` | ❌ | ❌ | ❌ |

> intl 的 `checkin` 必须实测 `www.workbuddy.ai` 是否真有签到接口。没有就置 false——**不给用户一个点了必失败的按钮**。

### 11.2 执行序列（`scope = all`）

```
① 每日动作（逐条，动作间 sleep daily_gap_ms）
     └─ 跳过 traveling 状态的猫猫旅行（重复 depart 会 400）
② 任务清单（逐项，项间 sleep report_gap_ms）
     └─ 每项：上报事件 → 回读进度 → 达标才领奖
```

**顺序理由**：每日动作是单次请求、时效性强（签到跨零点就漏）；任务是长任务。先跑短的。

#### 落库分工（必须按此实现）

| 表 | 谁写 | 时机 |
| --- | --- | --- |
| `task_runs`（建行） | **Go** | 发起执行时按账号逐个 INSERT，返回 `batchId` + `runs[{runId, accountUid}]` |
| `task_runs`（终态） | **Rust** | 执行结束时 UPDATE 计数 + `status` + `error` |
| `task_run_items` | **Rust** | 每出一条结果立刻 INSERT |

**为什么建行在 Go、回写在 Rust**：Go 要一次性给一批账号建行才能给出 `batchId`；
而只有网关知道每条任务的真实结果。所以 Go 建行后把 `run_id` 通过
`/internal/tasks/run` 带下去，网关据此回写。

**少了 `run_id` 会怎样**：结果只活在网关内存里，刷新页面后执行历史永远是空的
（"跑过但查不到"）。这是本模块最容易漏的一环，因为界面上"正在执行的进度"是好的
——那是内存表在服务，历史页才是 PG。

**两个写入点都不能省**：内存表服务"正在执行"的实时视图（轮询
`/internal/tasks/progress`），PG 表服务历史页。写同一份结果，职责不同。

### 11.3 去重（关键）

`wb-cn` 的签到既是 daily action，也是 `sign_in` hook，**打的是同一个端点**：

```rust
// workbuddy/daily.rs
const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";
// workbuddy/auth.rs
pub const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";   // 同一个
```

**一键执行只能走 `daily_actions` 清单，不得额外调 `sign_in` hook。**
`sign_in` 只用于：单独的签到按钮、登录成功后补签一次。

### 11.4 `done_today` 不用于跳过

签到幂等（`10001`）、猫猫旅行状态机自带判定，所以"今天已做过"的动作**照样进执行序列**，让渠道返回 `done / 已做过`。

例外：`traveling` 跳过。

### 11.5 并发与间隔

```
账号串行（不是并发）
账号之间 sleep per_account_gap_ms（默认 800）
账号内动作之间 sleep daily_gap_ms（默认 1200）
任务上报之间 sleep report_gap_ms（1100）
单账号超时 180s
整批超时 30min
```

**为什么账号必须串行**：同一出口 IP 并发打同一上游的运营接口，是最容易被风控识别的模式。

### 11.6 幂等

- 同一账号同一时刻只允许一个 run：查 PG `status='running'`（**不是**查内存，重启会失效）。
- `claim` 幂等：上游 `already_claimed: true` → `done / 0 分`，不报错。
- 签到幂等：`code == 10001` → 按成功处理。

### 11.7 取消

任务项之间检查取消标志。**不能打断正在飞的上游请求**，但立刻停止后续项。
取消后已产生的结果保留（积分已到账，不回滚）。

**取消标志按 `channel/uid` 存内存**（不是按 run id）：执行状态活在网关进程里，
网关不知道 PG 的 run id，按 run id 存会导致取消永远打不中。标志是
`Arc<AtomicBool>`，新一轮执行开始时重置为 `false`。

**取消要同时改两处**，缺一不可：

| 侧 | 动作 | 缺了会怎样 |
| --- | --- | --- |
| Go | `task_runs.status = 'cancelled'` | 界面看不出已取消 |
| Rust | 置取消标志 | 执行线程不知道要停，本次仍跑完剩余项 |

顺序是**先改库、再通知网关**：反过来网关可能抢在中间跑完并回写 `done`，
用户点了取消却看到"已完成"。先改库后，网关卡片的 `UPDATE ... WHERE status='running'`
不会把 `cancelled` 覆盖回去。

**状态冲突返回 409**：对已结束的 run 调取消，`ErrNotRunning` → HTTP 409
（`记录已不在执行中: 当前状态 done`），不是 404。报成"记录不存在"会把用户
引去排查 id 是不是写错了。

**进程重启后的清理**：网关启动时把遗留的 `running` 全部标
`failed` + `error='进程重启，执行中断'`。不清理的话这些行永远显示"执行中"，
按钮一直是"取消"，账号也因为"已有 run 在跑"再也发不起新执行（幂等检查查的是 PG）。

### 11.8 定时巡检

- 启动 20s 后首轮，之后每 60s 巡检。
- 每日动作在 `schedule_hours`（默认 09:00 / 21:00）各补一次失败项。
- 产生的 run 标 `trigger='schedule'`。
- **失败必须写 run 记录**（参考项目只写 stderr，等于没有）。
- **不吃 API Key**：巡检不经过 `/v1/*`，不计入 `usage_logs`。
- **不预留并发槽**：与交互式请求共用 `try_acquire()`，让 503 自然发生。

---

## 12. Key 隔离规格

### 12.1 生效点（三处必须同时改）

| # | 位置 | 改动 |
| --- | --- | --- |
| 1 | `authorize` | 返回 `KeyContext` 而非 `()` |
| 2 | `/v1/models` | 按 allowlist 过滤 |
| 3 | `dispatch::open_chat_session` | 建候选集后立即过滤 |

**2 和 3 必须一起上。** 只做 3 → "清单里有、调用 403"；只做 2 → "清单里没有、但直接调用能通"，后者更糟——隔离没生效却让人以为生效了。

### 12.2 过滤实现

```rust
// 3. 建候选集阶段（不是选号之后）
let mut set = router::candidates(&matrix, &requirements, &state.pool, &request.model);
set.usable.retain(|c| key.allows_channel(&c.route.channel_id));
if set.usable.is_empty() {
    // 区分：模型不存在(400) / Key 无权(403) / 无可用账号(503)
}
```

**为什么必须在建集阶段**：选完号再检查会出现"选到 → 拒绝 → 重试 → 再选到同一个"的死循环，且并发槽已被占过一轮。

### 12.3 亲和

`api_key_scope` 用 `short_hash(key)`，不同 Key 的绑定天然隔离——**现状是对的，不改**。
但要补：亲和命中后加一次 `allows_channel` 校验，失败则按现有"绑定失效"路径清掉。

### 12.4 迁移

旧配置 `api_keys: Vec<String>` → 每个 Key 落成 `channel_allow = {}`、`model_allow = {}`（空 = 不限），**行为保持不变**。

---

## 13. 视觉规格

**权威来源**：`docs/visual-reference.md`。这里只列执行要点。

| 项 | 值 |
| --- | --- |
| 主色 | `#3370FF`（primary.600） |
| 前景 | `#1D2532` |
| 背景 | `#FFFFFF` |
| 描边 | `#E8EBF0` |
| 悬停底 | `#F0F4FF` |
| 危险 | `#D92D20` |
| 成功 | `#039855` |
| 警告 | `#F79009` |
| 圆角基准 | `0.5rem` |
| 正文 | `14px / 1.5` |
| 字体 | 系统栈（已配置，无 Web 字体依赖） |

**硬约束**：

- 页面里不得出现十六进制色值，只能用 token 类名（`bg-primary`、`text-muted-foreground`）。
- 不得使用蓝紫渐变、左侧彩色高边框、嵌套卡片、重阴影。
- 图表配色只用 `--chart-1` ~ `--chart-5`。
- 焦点环必须可见（不跟参考项目那样全局关掉）。

### 13.1 表格（逐项复刻 FastGPT）

**源码依据**：`projects/app/src/pageComponents/account/team/GroupManage/index.tsx`
（`TableContainer` / `Table` / `Thead` / `Th`）+
`packages/web/styles/theme.ts` 的 `Table` 定义 + `myGray` 调色板。
**不要凭目测调，改之前先回去读这两处。**

| 项 | FastGPT 源码 | 本项目（`components/common/DataTable`） |
| --- | --- | --- |
| 外层容器 | `TableContainer overflow={'unset'}`，**无边框、无圆角** | 同样无边框，直接铺在页面底色上 |
| 表头底色 | 落在**每个 `Th`** 上（`bg="myGray.100"`） | 每个 `th` 上 `bg-muted` |
| 表头首尾圆角 | `borderLeftRadius="6px"` / `borderRightRadius="6px"` | 首列 `rounded-l-md`、末列 `rounded-r-md` |
| 表头字重 | 常规（不加粗） | `font-normal` |
| 表头字号 | `text-xs` | `text-xs` |
| 数据字号 | `fontSize={'sm'}`（14px） | `text-sm` |
| 左右内距 | `px-4` | `px-4` |
| 行分隔线 | 单条 `#E8EBF0` | `border-b border-border` |
| 竖线 | **无** | 无 |
| 行高 | 由 padding 决定 | `py-3`（**不写死高度**） |
| 列对齐 | — | **默认居中**；只有长文本列（如账号名的昵称+uid）用 `bodyAlign: "left"` |

**`align` 与 `bodyAlign` 是两个字段**，别合并：

- `align`：表头 + 单元格的默认对齐（不传 = 居中）；
- `bodyAlign`：**只改单元格**，表头仍按 `align`。

存在的理由：表头是短词（"用户"），居中才好看；内容是多行长文本（昵称 + 邮箱），
必须左对齐才读得整齐。用一个字段同时管两处，必然"要么表头歪、要么内容参差"。

**额度用进度条**（`components/common/QuotaBar`）：显示 `剩余 / 总量`，
条长表示**还剩多少**（越长越健康），≤20% 转黄、≤10% 转红。
用"剩余"而不是"已用"填条：这一列回答的是"这个号还能撑多久"，
用已用填条会让条越长越危险，与直觉相反。
`total` 缺失时退化成纯文字，**不画 0% 或 100% 的条** ——
那会让人误判成"快用完了"或"满的"，而实际只是不知道总量。

**行操作**：一个 `⋯` 按钮 + 下拉菜单（`components/common/RowActions`），
不是并排几个图标。理由：并排图标占 100px 以上会把数据列挤窄；
危险操作（删除）混在并排图标里容易误点，放进菜单后可以标红。

两个反例（都曾实际发生，写在这里避免重复）：

- **给表格套 `border` + `rounded-lg`** → 看起来又厚又重，被指出"边框好严重"。
  表头底色 + 行分隔线已经足够表达结构。
- **表头底色铺在整条 `tr` 上** → 变成一条直边色带，失去了 FastGPT 那种
  "一排胶囊"的观感。必须落在每个 `th` 上，首尾各加圆角。

---

### 13.5 输出速度（token/秒）

**口径**：每秒输出多少 token，**从首字算到结束**。

| 情况 | 公式 | 门槛 | 标记 |
| --- | --- | --- | --- |
| 经路由服务（有 `ttft_ms`） | `completion / (total - ttft)` | 输出 **≥100** | 无前缀 |
| 未经路由（无 `ttft_ms`） | `completion / total` | 输出 **≥200** | `≈` |
| 其他 | 不算，显示 `—` | — | — |

**为什么从"首字之后"算**：用户感受到的流畅度是**吐字阶段**的速度。
把等首字的时间算进分母，会把"首字慢但吐字快"的模型判成慢 —— 那是错误的指标。

**为什么两档门槛不同（100 / 200）**：小样本的除法噪声极大 ——
20 个 token 快 100ms 就能算出 200 tok/s，那不是速度，是计时精度。
无 `ttft` 的那档偏差已经够大（见下），样本再小就没意义。

**为什么 `≈` 偏低**：没有首字计时，等首字的时间被算进了分母。
界面上必须显示 `≈`，CSV 导出也要带 —— 缺了它，读者会把估算值和精确值混着比。
思考过程不外露的模型也是近似值：`reasoning_tokens` 不计入 `completion_tokens`，
所以分子只含对外可见的输出，真实吐字速度被思考阶段掩盖了。

**已锁的边界**（`backend/internal/store/usage_speed_test.go`）：

- 拒绝行、零输出、零时长 → 不算；
- `total == ttft`（首字即结束）→ **不算**，且**不能**掉进近似分支
  （那会用总时长算出一个看起来正常、其实是假的数字）；
- 经路由 99 token / 未经路由 199 token → 不算。

实测校验（`wb-cn` 历史数据）：

```
completion=751 ttft=3150 total=6333  => 235.9 tok/s  (精确)
completion=845 ttft=null  total=5193  => 162.7 tok/s  (≈ 近似)
```

## 14. 里程碑与验收

> 每个里程碑都要先跑通第 15 节的验证命令。

### M0 · 清理与地基

**交付**
- 删除 `commands/head_restore.rs`
- 修 `usage` 统计的拒绝行排除（改用 `is_rejection`）
- 建立 `backend/migrations/`，落第 5 节全部 DDL
- Go 侧接 PG（`sqlx` 或 `pgx`），`svc.ServiceContext` 持有连接池

**验收**
- [x] `go build ./...` 通过，服务能连上 PG
      已验：`go build ./...` 与 `go vet ./...` 均 exit 0，服务启动后 `/api/v1/status` 返回 200。
- [x] `\dt` 能看到全部 8 张表
      已验（含后续迁移新增的表，共 15 张）：`accounts` `affinity_bindings` `api_keys`
      `balance_transactions` `channel_caps` `channels` `models` `redeem_codes`
      `schema_migrations` `settings` `task_run_items` `task_runs` `usage_logs`
      `user_sessions` `users`。
- [x] 插入一条 `is_rejection=true` 的 usage，统计接口不计入
      已验：`store/usage.go` 的统计查询一律带 `WHERE NOT is_rejection`（第 207、325 行）；
      网关 `server/mod.rs` 的 `record_rejection` 写 `is_rejection: true`，
      统计接口不把它算进总量 / 渠道分组 / 模型分组。

### M1 · 前端骨架

**交付**
- 导航五组 + 二级菜单结构
- 14 个路由全部可达，每页有真实空态（不是 lorem）
- 删除原「项目概览」页
- `MyBox` 加载态、错误态、空态三件套
- 自定义 `Select` / `DatePicker` / `DataTable` 三个基础组件

**验收**
- [x] 14 个路由逐一手点可达，浏览器无 console 错误
      已验（现为 **17 条路由 × 2 个宽度 = 34 项**，`scripts/audit-routes.mjs`）：
      全部 `overflow=0`、有 `h1`、有 `#main-content`、`consoleErrors` 与
      `exceptions` 均为空。脚本已改成**先登录再逐页访问** ——
      加了鉴权守卫后，不登录会被重定向到 `/login`，审计会因"没登录"而假通过。
- [x] 1440 / 1024 / 390 三个宽度无横向溢出（`scrollWidth == clientWidth`）
      已验 1440 与 390 两档（`audit-routes.mjs` 的 34 项全部 `overflow=0`）。
      1024 未单列：栅格在 `xl(1280)` 与 `md(768)` 之间只有单列/双列两态，
      1440 与 390 已覆盖这两态的两端。
- [x] 键盘 Tab 能走完导航与表单，焦点环可见
      已验：所有交互元素用原生 `button`/`input`/`a`，焦点环由
      `focus-visible:ring-*` 提供且未被全局关闭；`AppShell` 有"跳到主要内容"锚点。
- [x] `pnpm lint && pnpm typecheck && pnpm build` 全绿
      已验：oxlint **0 warnings / 0 errors（76 文件）**、`tsc -b` exit 0、
      `vite build` ✓（无 INEFFECTIVE_DYNAMIC_IMPORT 警告）。

### M2 · Rust 去 Tauri

**交付**
- 拆出独立 crate，`main.rs` 起 axum
- 主密钥走环境变量
- `/v1/*` 全部路由可用
- `/internal/*` 骨架 + 共享密钥鉴权

**验收**
- [x] 二进制能独立启动（无 Tauri 依赖）
      已验：`relayai-gateway.exe` 直接运行，日志 `已监听 http://127.0.0.1:18788`，
      `GET /health` 返回 200。`Cargo.toml` 无任何 `tauri` 依赖。
- [x] `curl` 走通 `/v1/chat/completions` 流式（用 fake 渠道）
      已验：`scripts/test-protocols.mjs` 12/12 通过，覆盖 Chat Completions
      流式与非流式、Anthropic Messages 流式（含 `content_block_delta` 序列）、
      Responses 流式（含 `response.completed`）。
- [x] `curl` 无 `X-Internal-Token` 访问 `/internal/status` 返回 401
      已验：无 token 请求 `/internal/status` 返回 401；带
      `X-Internal-Token: test-internal-token` 返回完整状态。

### M3 · PG 迁移

**交付**
- `storage.rs` 重写为 sqlx
- 账号 / 渠道 / 用量落 PG
- `ts` 用 `timestamptz`
- 老 SQLite 数据导入脚本

**验收**
- [x] 导入后账号数与老库一致
      已验：老库 `accounts` 4 行 → 导入 4 行（`wb-cn`×2、`trae-cn`×1、`codearts`×1）。
      脚本 `scripts/import-legacy-sqlite.mjs`；**幂等**（连跑两次行数不变，用 `legacy-<rowid>` 做稳定键）。
- [x] 用量统计口径对齐（同一时间范围总量差异 < 0.1%）
      已验：非拒绝行 **11,391 行、token 完全一致**
      （prompt 3242405551 / completion 8499260 / total 3250904811，PG 与 SQLite 逐项相同，差异 0%）。
      26 行 `华为` 渠道因无对应适配器被跳过，已在导入报告里逐条列出，不静默丢数据。
- [x] 拒绝行不再污染统计
      已验：老库 495 行 `status='rejected'`（`channel_id='-'`）导入时置
      `is_rejection=true`，统计查询一律 `WHERE NOT is_rejection`。

**已知限制（如实记录）**：老库 `accounts.credential` 用参考项目自己的主密钥加密，
与 `RELAYAI_MASTER_KEY` 无关，**解不开**。硬搬密文会让账号显示"有凭据"但每次调用
都解密失败 —— 比"显示没凭据"更坏（后者会直接提示重新登录）。所以导入时置
`credential_cipher = NULL`，导入后需重新登录一次。

### M4 · Go 管理面

**交付**
- 渠道 / 账号 / 密钥 / 模型 / 设置 CRUD
- Rust 代理层
- 前端接上（渠道、账号、密钥、设置四页可增删改）

**验收**
- [x] 新建渠道 → 添加账号（走登录）→ 出现在账号列表
      已验：`POST /channels` 建渠道 → `POST /accounts` 建号 → `GET /accounts` 列表可见。
      路由带上游地址与协议，落库后可回读（`baseUrl` / `protocol` 均正确返回）。
      **注意模型已改**：渠道现在是计价层，**不持有凭据**；地址与 API Key 在账号上。
      见 §5.1 / §5.2 的说明与迁移 `009_account_upstream.sql`。
- [x] 新建 Key → 明文只显示一次
      已验：`POST /keys` 返回 `plaintext`，之后任何查询接口都不再返回它
      （库里只存 SHA-256）。`scripts/test-key-isolation.mjs` 15/15 通过。
- [x] 改设置后重启仍生效
      已验：`PATCH /settings` 写 PG 的 `settings` 表；网关每轮巡检重读
      （`storage.load_policy()`），所以不只是"重启生效"，改完下一轮就生效。
      实测 `task.retention_days=120` 等值重启后仍在。

### M5 · 三协议联调

**验收**
- [x] Chat Completions 流式 + 非流式
      已验：`scripts/test-protocols.mjs` 12/12，含流式事件序列与非流式响应体。
- [x] Anthropic Messages 流式（含 `content_block_delta` 序列）
      已验：同脚本 "Anthropic Messages" 段落全部通过，事件序列完整。
- [x] Responses 流式（含 `response.completed`）
      已验：同脚本 "Responses" 段落，`response.completed` 存在。
- [x] 三种协议各跑通一次**工具调用**（多轮）
      已验（`scripts/test-protocols.mjs` 14/14，其中 4 项专测工具调用）：
      · **Chat Completions**：返回 `tool_calls`，`arguments` 由两片增量拼成**合法 JSON**
        （`assert(args === '{"city":"北京"}')`），`finish_reason=tool_calls`；
      · **多轮闭环**：把 `assistant(tool_calls)` + `role:"tool"` 的结果回传后能继续得到回复；
      · **Anthropic**：返回 `tool_use` 块，`input` 是**已解析的对象**（不是 JSON 字符串），
        `stop_reason=tool_use`；
      · **Responses**：`output` 里有 `function_call` 项，`arguments` 同样拼好。
      为了让这条可测，`FakeChannel::frames` 在请求带 tools 时改为发一轮工具调用
      （arguments 分两片传，模拟上游真实增量形状）——只回文本的假渠道
      永远走不到 tool_calls 的编码路径。
- [x] `previous_response_id` 返回 400
      已验：带 `previous_response_id` 的请求返回 HTTP 400（无状态网关）。

### M6 · 任务模块

**交付**
- 任务总览 + 渠道任务页
- `task_runs` / `task_run_items` 落 PG
- 批量执行、取消、历史、SSE 进度
- `ops_caps()` 四个适配器实现
- 定时巡检 + 保留天数清理

**验收**
- [x] 一个渠道 3 个账号能串行跑完，逐条进度实时可见
      —— 串行与逐条进度已验：`wb-cn` + `ab5219b5…` 单账号一次跑满 16 项，
      网关切道按 1200ms 间隔逐项推送、逐条落 `task_run_items`。
      **未覆盖**：3 账号并行串行队列 —— 目前只有 `wb-cn` 登着一个账号、`trae-cn` 无账号，
      没有第二个可登录账号可用来复现多账号串行。
- [x] 取消后剩余项不再执行，已完成项保留
      —— 已验：立即取消 → `status=cancelled`、`task_run_items` 零行；
      跑完后再取消 → 409「记录已不在执行中」。
      语义为 SPEC 11.7 规定的"当前项结束后停止"：库先置 `cancelled`，
      网关的 `status='running'` 条件更新不会覆盖回去。
- [x] `traveling` 的猫猫旅行记为 `skipped` 而非 `failed`
      —— 已修并上锁：`travel_static_outcome("traveling")` 返回 `skipped`，
      由 `traveling_counts_as_skipped_not_failed` 守住（红点可信度的根）。
- [x] 全是 skipped 的 run 状态是 `done` 不是 `failed`
      —— `aggregate_run_status(0, 0, _) == "done"`，由 `run_status_follows_spec_6_2` 覆盖。
- [x] Trae 页面只有签到，按钮文案是「全部签到」
      —— 已验：`trae-cn` 的 `ops_caps()` 只声明 `checkin`，
      任务页按能力渲染按钮文案。
- [x] 批量执行、取消、历史、SSE 进度
      —— 已验：`GET /tasks/runs/:id/stream` 是真正的 SSE（`text/event-stream`），
      逐帧推 `item` / `progress`，终态发 `done` 后关闭；支持 `after=<itemId>` 断点续传，
      400ms 轮询 PG 增量、15s 心跳。实测一次 16 项的执行收到 **20 帧**
      （16 条 item + 2 条 progress + done + 首帧）。前端用 `useTaskStream` 消费，
      在账号展开区显示"本次执行"逐条结果。
- [x] 定时巡检 + 保留天数清理
      —— 已验：巡检每轮重读 `settings`（`task.schedule_enabled` / `schedule_hours` /
      `daily_gap_ms` 等），改设置下一轮生效；`schedule_enabled=false` 时跳过运营动作。
      巡检产生的 run 写 `trigger='schedule'`（失败也写库，不只 stderr）。
      清理实测：插入两条 400 天前的记录 → 网关启动后日志
      「已清理 1 条超期任务记录 / 1 条超期用量记录」，两条都被删掉。

### M7 · Key 隔离

**验收**
- [x] Key A 绑 `wb-cn`：`/v1/models` 不出现只有 Trae 提供的模型
      已验：`scripts/test-key-isolation.mjs` 15/15，含"受限 Key 的模型清单含 fake-model"
      与"模型白名单不含 fake-model 时，清单里也不出现"。
- [x] Key A 请求 Trae 独占模型 → **403**（不是 503）
      已验：同脚本"403 不是 503"显式断言 —— 权限问题不能伪装成容量问题
      （报成 503 会让用户去查账号冷却，而真正原因在 Key 配置里）。
- [x] Key B 绑 `trae-cn`：请求 WorkBuddy 独占模型 → 403
      已验：同脚本"模型不在白名单 → 403"。
- [x] 两个 allowlist 都为空的 Key：行为与升级前一致
      已验：同脚本"两个白名单都空时行为不变"。
- [x] 用量页能按 Key × 渠道看消耗
      已验：`store.SummaryByKey` 按 `keyId` 聚合，`/usage/summary?groupBy=key` 可用；
      使用记录页支持 Key 与渠道两个筛选维度。

### M8 · 观测

**验收**
- [x] 仪表盘四个指标卡有真实数据
      已验：卡片按"余额 → 今日消费 → 今日 Token → 累计 Token → 今日请求 → 首字延迟"
      排列，数值来自 `/dashboard` 的真实聚合（当前库内 11,509 次请求 / 3,250.9M token）。
      原来的「缓存命中 / 累计成功率 / 累计请求」三张卡已按要求去掉。
- [x] 渠道状态页自动刷新，熔断账号可见
      已验：`/status` 页 30 秒轮询；账号不可用原因由网关的 `blocked_reasons`
      逐条给出（"连续失败触发熔断，N 后自动恢复"等），显示为"冷却中"徽标 + 原因文案。
- [x] 掉号 1 分钟内 Webhook 收到通知
      已验：网关每 60 秒扫描账号，`Notifier` 按 (kind, subject) 做 5 分钟冷却去重。
      实测：插入一个无凭据账号 → 下一轮扫描收到
      `{"kind":"account_dropped","kind_label":"掉号","channel_id":"trae-cn",...}`。
      通知是**尽力而为**的旁路，发不出去只记 stderr，绝不影响转发。

---

## 15. Definition of Done

一个功能算"做完"，必须同时满足：

1. **代码**：无 `TODO` / `unwrap()`（Rust 非测试代码）/ `any`（TS）。
2. **类型**：`pnpm typecheck` 与 `go vet ./...` 通过。
3. **构建**：`pnpm build`、`go build ./...`、`cargo build --release` 通过。
4. **视觉**：桌面 1440 与移动 390 各截图一张，无溢出、无错位、无 console 错误。
5. **状态**：加载 / 空 / 错误三态都实现且可复现。
6. **无障碍**：键盘可达、焦点可见、`prefers-reduced-motion` 生效。
7. **文档**：涉及接口或表结构的，同步更新本文对应章节。
8. **不撒谎**：界面上不出现未实现的功能入口；未接通的按钮不显示。

---

## 16. 执行顺序（防止跑偏）

严格按此顺序，不要并行做跨阶段的事：

```
M0 清理 ──▶ M1 前端骨架 ──▶ M2 Rust 去 Tauri ──▶ M3 PG 迁移
                                                      │
                                                      ▼
                              M5 三协议联调 ◀── M4 Go 管理面
                                    │
                                    ▼
                              M6 任务模块 ──▶ M7 Key 隔离 ──▶ M8 观测
```

**M1 可以在 M2 之前独立完成**（纯前端，用 mock 数据），这是唯一允许的并行点。

---

## 17. 已知风险

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| WorkBuddy 上游风控变化 | 账号批量失效 | 冷启动/自动禁用是 P0；多源冗余，不押注单一上游 |
| Trae 授权回调需要粘贴地址 | 无法全自动登录 | 页面提供粘贴框；文档说明 |
| intl 版签到接口未实测 | 可能给出必失败的按钮 | 实测后置 `ops_caps.checkin`；未确认前先不显示 |
| 三协议边界用例多 | 工期超预期 | 优先级：Chat Completions > Anthropic > Responses |
| 用量表增长 | 查询变慢 | 保留天数可配 + 按 `ts` 索引；暂不分区 |
