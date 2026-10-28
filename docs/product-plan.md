# relayAI 产品与架构设计（v0.2）

> 状态：**设计稿**，尚未落到代码。仓库仍为骨架（AppShell + 路由 + `/health`）。
>
> **⚠ 执行以 [SPEC.md](./SPEC.md) 为准。** 本文是背景与论证，冲突时以 SPEC 为准。
>
> **⚠ 先行阅读**：参考项目已盘点完毕，第 4 节「三协议兼容」与第 7 节「后端分层」已被
> [migration-plan.md](./migration-plan.md) 取代——协议互转已经实现，不需要重写。
>
> v0.2 相对 v0.1 的变更：
> 1. 定位收敛为**自用工具**（单人 / 小团队），去掉注册、充值、多租户、分组倍率等运营能力；
> 2. 网关协议入口确定为**三形态**：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages；
> 3. 渠道侧来源为 WorkBuddy / Trae / 自定义渠道商，三类统一走"适配器"抽象；
> 4. 计费确定为**倍率制**（上游成本 × 倍率 = 记账额度）；
> 5. 数据库确定为 **PostgreSQL**，并给出建表草案与并发约束。

---

## 0. 已确认的决策

| 决策 | 结论 | 影响 |
| --- | --- | --- |
| 使用场景 | **先自用** | 无注册、无充值、无多租户；但保留"多个 API Key"以便区分客户端/项目 |
| 上游来源 | WorkBuddy / Trae / 自定义渠道商 | 三者都是"渠道"，凭据形态不同，走同一套 CRUD |
| 上游实现 | 已有反代项目（Tauri + Rust） | 三协议入口与调度已实现，迁移方案见 [migration-plan.md](./migration-plan.md) |
| 协议入口 | Chat Completions + Responses + Anthropic Messages | 三向互转**已在参考项目实现**，relayAI 直接复用，见迁移方案 |
| 计费 | 倍率 | `cost = upstream_cost × ratio`；比价与记账分离，见第 6 节 |
| 数据库 | PostgreSQL | 用 JSONB 存模型映射/凭据元数据，用 partial index 做冷却队列 |

---

## 1. 三条主线

```
客户端 (Cursor / Claude Code / Cherry Studio / 脚本)
    │  三种协议任选
    ▼
┌─────────────────────────────────────────────┐
│ 接入层  /v1/chat/completions                 │
│         /v1/responses                        │
│         /v1/messages                         │
│  → 解析成统一的 InternalRequest              │
└─────────────────────────────────────────────┘
    ▼
┌─────────────────────────────────────────────┐
│ 调度层  选渠道 → 选账号 → 并发闸门 → 粘性    │
│         失败重试 / 换号 / 熔断 / 降级        │
└─────────────────────────────────────────────┘
    ▼
┌─────────────────────────────────────────────┐
│ 适配层  workbuddy | trae | custom(openai兼容)│
│  → 出站请求 → 上游 → 统一事件流              │
└─────────────────────────────────────────────┘
    ▼
┌─────────────────────────────────────────────┐
│ 记录层  usage_logs / 账号健康 / 熔断状态      │
└─────────────────────────────────────────────┘
```

**关键点：适配层输出的是"统一事件流"，不是裸 SSE。** 接入层再把统一事件流编码成客户端要的协议。这样 3 种入口 × 3 种上游 = 9 种组合，只需写 3 + 3 个转换器，而不是 9 个。

---

## 2. 信息架构（自用版导航）

自用版砍掉注册/充值/兑换码，导航压到五组：

### 概览
| 路由 | 页面 | 核心内容 |
| --- | --- | --- |
| `/dashboard` | 仪表盘 | 今日请求/Token/成本、成功率、P95 延迟、活跃账号、异常渠道、错误 Top5、24h 趋势 |

### 渠道与账号
| 路由 | 页面 | 核心内容 |
| --- | --- | --- |
| `/channels` | 渠道管理 | 列表（类型、状态、权重、模型数、今日调用）；新建/编辑抽屉 |
| `/channels/:id` | 渠道详情 | Tab：账号池 / 模型映射 / 用量与错误 |
| `/accounts` | 账号管理 | 跨渠道全量视图，按状态/渠道/类型筛选；额度余量、冷却剩余、最近失败原因 |
| `/accounts/:id` | 账号详情 | 凭据有效期、额度窗口曲线、最近 50 次请求、操作审计、手动探活/重登 |
| `/status` | 渠道状态 | 可用率、探测延迟、熔断三态、冷却中账号、上游错误分布 |

### 用量
| 路由 | 页面 | 核心内容 |
| --- | --- | --- |
| `/usage` | 使用记录 | 请求级日志：时间、Key、渠道、账号、模型、Token、首字延迟、总耗时、状态、重试次数 |
| `/usage/:requestId` | 请求详情 | 完整重试链、上游错误原文（脱敏）、协议转换耗时 |
| `/stats` | 统计 | 按模型/渠道/账号/天聚合，导出 CSV |

### 任务（一级菜单，二级为各渠道）
| 路由 | 页面 | 核心内容 |
| --- | --- | --- |
| `/tasks` | 任务总览 | 按渠道聚合的今日待办、运行中批次、最近失败；渠道级一键执行入口 |
| `/tasks/:channelId` | 渠道任务 | 账号表（今日进度/状态/最近执行）+ 展开看任务清单与每日动作，逐条执行/领取/取消 |

> 详细设计见 [task-module.md](./task-module.md)。任务**不挂在账号页下**，账号详情只留一个跳转链接。

### 系统
| 路由 | 页面 | 核心内容 |
| --- | --- | --- |
| `/keys` | 我的密钥 | 每条绑**渠道白名单 + 模型白名单**、限速、备注；详情见 [key-isolation.md](./key-isolation.md) |
| `/settings` | 设置 | 全局默认渠道策略、探活间隔、**任务记录保留天数**、**定时巡检开关**、通知 Webhook |

> 自用版**不做**：用户管理、管理员角色、注册登录（用单个管理员口令 + 本地会话）、计费充值、审计日志（先用结构化日志文件，不做页面）。
> 但 `/keys` 保留——你需要区分"Cursor 用的 key"和"脚本用的 key"，否则用量记录混成一团。
> Key 的**渠道隔离**必做：不做的话，一个 Key 能顺着模型矩阵落到任何渠道，烧光你所有订阅额度却查不出是谁烧的。

---

## 3. 核心概念

```
渠道 Channel    = 怎么跟上游说话（适配器类型 + 端点 + 模型映射 + 倍率 + 策略）
  └── 账号 Account = 渠道下的一个凭据实例（cookie / token / key），池子成员
        └── 请求 Request = 一次调用落在某个账号上，产生一条 usage_log
密钥 Key        = 你自己发给客户端的 sk-xxx，绑定渠道白名单 + 模型白名单 + 限速
```

> **「渠道」和「账号」的边界要用一句话记住**：渠道回答"用什么协议、什么价格"，
> 账号回答"用谁的额度、还能不能用"。Key 回答"谁在用、能用哪些渠道"。

### 渠道类型

| type | 说明 | 凭据 | 需要的能力 |
| --- | --- | --- | --- |
| `workbuddy` | 登录态反代 | cookie / refresh token | 刷新、探活、额度窗口探测 |
| `trae` | 登录态反代 | 同上 | 同上 |
| `custom` | 自定义渠道商，OpenAI 兼容 | api key + base_url | 探活、余额查询（可选） |

> 三类共用 `accounts` 表，用 `channel_id` 归属；凭据用 JSONB 存，因为形态差异大（cookie 串 / token + 设备 id / 纯 key）。加密后整体存入 `credential_cipher`，明文只在内存中出现。

---

## 4. 三协议兼容（本次最大的设计点）

### 4.1 三种入口的差异

| 维度 | Chat Completions | Responses | Anthropic Messages |
| --- | --- | --- | --- |
| 路径 | `/v1/chat/completions` | `/v1/responses` | `/v1/messages` |
| 消息结构 | `messages[{role, content}]`，content 可为 string 或 parts 数组 | `input`（string 或 items 数组）+ `instructions` | `system` 独立字段 + `messages` |
| 系统提示 | `role: "system"` 消息 | `instructions` | 顶层 `system` |
| 工具调用 | `tools[{type:"function"}]`，回填 `tool_calls` + `role:"tool"` | `tools` + `function_call` / `function_call_output` items | `tools` + `tool_use` / `tool_result` content block |
| 流式 | `data: {...}` 增量 delta | 具名事件 `response.output_text.delta` 等 | 具名事件 `message_start` / `content_block_delta` / `message_stop` |
| 结束标记 | `data: [DONE]` | `response.completed` | `message_stop` |
| 用量 | `usage{prompt,completion}` | `usage{input_tokens,output_tokens}` | `usage{input_tokens,output_tokens,cache_*}` |
| 思考过程 | `reasoning_content`（非标准） | `reasoning` item | `thinking` content block |
| 缓存计费 | 部分支持 | 部分支持 | `cache_creation_input_tokens` / `cache_read_input_tokens` |

### 4.2 统一内部模型

```go
type InternalRequest struct {
    Model        string
    System       []ContentBlock   // 统一成 blocks，便于三向转换
    Messages     []Message        // role: user|assistant|tool
    Tools        []Tool
    ToolChoice   any
    MaxTokens    int
    Temperature  *float64
    Stream       bool
    // 协议元信息，用于回程编码
    OriginProtocol Protocol       // chat|responses|anthropic
}

type ContentBlock struct {
    Type     string   // text | image | tool_use | tool_result | thinking
    Text     string
    ImageURL string
    ToolUse  *ToolCall
    ToolResult *ToolResult
}
```

### 4.3 流式事件（适配层输出）

```go
type StreamEvent struct {
    Kind     string   // start | text_delta | thinking_delta | tool_call_delta | usage | done | error
    Text     string
    ToolCall *ToolCallDelta
    Usage    *Usage
    Err      *UpstreamError
}
```

接入层按 `OriginProtocol` 把 `StreamEvent` 编码成对应 SSE 格式。

### 4.4 三条硬规则

1. **能力不对等要显式降级，不能静默丢字段。** 例如客户端走 Chat Completions 要 `reasoning_content`，上游只给 Anthropic thinking——转换；反之若上游不支持，返回能力缺失而不是假装成功。
2. **工具调用 id 必须原样透传。** 三个协议的 `tool_call_id` 语义不同，转换时建映射表，回填时还原，否则多轮工具调用必崩。
3. **`/v1/responses` 是最容易被低估的。** 它是有状态的（`previous_response_id`、`store`）。自用场景建议**只实现无状态模式**：不保存响应、不支持 `previous_response_id` 续接，客户端每次传完整 `input`。这样能省掉一整张状态表。

### 4.5 工作量提示

三协议兼容的实际成本：
- Chat Completions → 统一模型：约 1 天
- Anthropic Messages → 统一模型：约 1.5 天（content block 更复杂）
- Responses → 统一模型：约 2 天（items 模型最绕）
- 反向编码（统一事件流 → 三种 SSE）：约 2 天
- 边界用例（多模态、并行工具调用、thinking、截断）：**这部分通常比上面加起来还多**

**建议顺序：Chat Completions 先通 → Anthropic → Responses。** 前两个覆盖 90% 客户端，Responses 最后补。

---

## 5. 数据库设计（PostgreSQL）

### 5.1 建表草案

```sql
-- 渠道
CREATE TABLE channels (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL UNIQUE,
    type              TEXT NOT NULL,              -- workbuddy | trae | custom
    base_url          TEXT NOT NULL,
    auth_scheme       TEXT NOT NULL,              -- cookie | bearer | header:<name>
    model_mapping     JSONB NOT NULL DEFAULT '{}',-- {canonical: upstream_model}
    capabilities      JSONB NOT NULL DEFAULT '{}',-- {stream,tools,vision,thinking}
    ratio             NUMERIC(10,4) NOT NULL DEFAULT 1.0,
    weight            INT NOT NULL DEFAULT 100,
    priority          INT NOT NULL DEFAULT 0,     -- 数字小优先
    status            TEXT NOT NULL DEFAULT 'active',
    rate_limit_qps    INT,
    concurrency_limit INT,
    probe_interval_s  INT NOT NULL DEFAULT 300,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 账号
CREATE TABLE accounts (
    id                BIGSERIAL PRIMARY KEY,
    channel_id        BIGINT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    label             TEXT NOT NULL,
    credential_cipher BYTEA NOT NULL,             -- AES-GCM 密文
    credential_meta   JSONB NOT NULL DEFAULT '{}',-- 非敏感元数据：账号标识、设备id哈希
    status            TEXT NOT NULL DEFAULT 'active', -- active|cooldown|invalid|banned
    weight            INT NOT NULL DEFAULT 100,
    concurrency       INT NOT NULL DEFAULT 3,
    quota_total       NUMERIC(14,4),
    quota_used        NUMERIC(14,4) NOT NULL DEFAULT 0,
    quota_reset_at    TIMESTAMPTZ,
    cooldown_until    TIMESTAMPTZ,
    fail_count        INT NOT NULL DEFAULT 0,
    last_ok_at        TIMESTAMPTZ,
    last_error        TEXT,
    credential_expires_at TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 选号热路径：只索引可用账号
CREATE INDEX idx_accounts_available ON accounts (channel_id, weight DESC)
    WHERE status = 'active' AND (cooldown_until IS NULL OR cooldown_until < now());

-- 密钥（自用版：只有自己用）
-- 两个 allowlist 都是"空 = 不限"；显式存渠道实例 id，不存适配器类型。
CREATE TABLE api_keys (
    id            BIGSERIAL PRIMARY KEY,
    name          TEXT NOT NULL,
    key_hash      TEXT NOT NULL UNIQUE,
    key_prefix    TEXT NOT NULL,
    channel_allow TEXT[],                      -- 渠道实例 id，如 {wb-cn,wb-intl}
    model_allow   TEXT[],                      -- 对外模型名
    rpm_limit     INT,
    daily_quota   NUMERIC(14,4),
    used_today    NUMERIC(14,4) NOT NULL DEFAULT 0,
    quota_reset_on DATE,
    expires_at    TIMESTAMPTZ,
    last_used_at  TIMESTAMPTZ,
    status        TEXT NOT NULL DEFAULT 'active',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 使用记录（大表）
CREATE TABLE usage_logs (
    id                BIGSERIAL PRIMARY KEY,
    request_id        TEXT NOT NULL UNIQUE,
    key_id            BIGINT REFERENCES api_keys(id),
    channel_id        BIGINT,
    account_id        BIGINT,
    protocol          TEXT NOT NULL,              -- chat|responses|anthropic
    model             TEXT NOT NULL,
    upstream_model    TEXT,
    prompt_tokens     INT NOT NULL DEFAULT 0,
    completion_tokens INT NOT NULL DEFAULT 0,
    cache_read_tokens INT NOT NULL DEFAULT 0,
    upstream_cost     NUMERIC(14,8) NOT NULL DEFAULT 0,
    ratio             NUMERIC(10,4) NOT NULL DEFAULT 1.0,
    billed_cost       NUMERIC(14,8) NOT NULL DEFAULT 0,
    first_token_ms    INT,
    total_ms          INT,
    status            TEXT NOT NULL,              -- ok|error|aborted
    error_type        TEXT,
    error_detail      TEXT,
    retry_count       INT NOT NULL DEFAULT 0,
    stream            BOOLEAN NOT NULL DEFAULT false,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_usage_created ON usage_logs (created_at DESC);
CREATE INDEX idx_usage_account ON usage_logs (account_id, created_at DESC);
CREATE INDEX idx_usage_channel ON usage_logs (channel_id, created_at DESC);

-- 模型目录 + 倍率
CREATE TABLE models (
    canonical_name TEXT PRIMARY KEY,
    display_name   TEXT NOT NULL,
    capabilities   JSONB NOT NULL DEFAULT '{}',
    context_window INT,
    input_price    NUMERIC(14,8) NOT NULL DEFAULT 0,  -- 每 1M token
    output_price   NUMERIC(14,8) NOT NULL DEFAULT 0,
    cache_price    NUMERIC(14,8) NOT NULL DEFAULT 0,
    ratio          NUMERIC(10,4) NOT NULL DEFAULT 1.0,
    status         TEXT NOT NULL DEFAULT 'active'
);
```

### 5.2 为什么这样设计

- **JSONB 用于 `model_mapping` / `capabilities`**：渠道能力差异大且会变，用 JSONB 避免每次加能力都改表。
- **partial index 做选号队列**：`WHERE status='active' AND cooldown 已过`，索引只包含可用账号，选号时几乎不扫无效行。
- **`usage_logs` 不建外键到 accounts**：账号会被删，但历史记录要保留，所以只存 id 值。
- **倍率落在 `models` 与 `channels` 两处**：模型级倍率是基准，渠道级倍率是修正（例如某渠道成本高，倍率上调）。最终 `billed = upstream_cost × model.ratio × channel.ratio`。
- **`upstream_cost` 与 `billed_cost` 分开存**：上游涨价、倍率调整后，历史记录仍可还原真实成本，对账不会失真。

### 5.3 待确认

- 是否用 **TimescaleDB 扩展** 或按月分区 `usage_logs`？自用量级下（每天几千条）普通表 + 索引足够，先不引入。
- 主密钥（加密凭据用）放环境变量还是 `settings` 表？**建议环境变量**，且启动时校验存在。

---

## 6. 倍率计费的具体口径

```
上游真实成本  upstream_cost = (prompt_tokens × input_price
                              + completion_tokens × output_price
                              + cache_read_tokens × cache_price) / 1_000_000
记账额度      billed_cost   = upstream_cost × model.ratio × channel.ratio
```

三个必须明确的口径问题：

1. **上游不返回 usage 怎么办？** 标记 `usage_estimated = true`，用本地估算（按字符数 / 分词器）。**不要静默按 0 记**，否则报表永远偏低。
2. **缓存命中怎么算？** Anthropic 的 `cache_read_input_tokens` 单价通常是输入的 10%，必须单独计价，否则成本会高估 5~10 倍。
3. **失败/中断的请求算不算？** 上游已产生 token 但客户端断开——按上游返回的 usage 记，`status='aborted'`，仍计费。这条不写清楚，账永远对不上。

---

## 7. 后端分层（go-zero）

```
backend/
├── backend.api            # 管理面 REST（/api/v1）
├── gateway/               # 网关面（/v1/*），不走 go-zero 的 api 描述文件
│   ├── ingress/           # 三协议解析 → InternalRequest
│   ├── egress/            # StreamEvent → 三协议 SSE
│   └── stream.go          # SSE 生命周期、客户端断开取消
├── internal/
│   ├── adapter/
│   │   ├── openai/        # custom 渠道
│   │   ├── workbuddy/
│   │   └── trae/
│   ├── router/            # 选号、重试、熔断、并发闸门、粘性
│   ├── billing/           # 倍率计算
│   └── logic/ handler/    # CRUD
```

适配器接口：

```go
type Adapter interface {
    Chat(ctx context.Context, req *InternalRequest, acct *Account) (<-chan StreamEvent, error)
    Probe(ctx context.Context, acct *Account) (Health, error)
    Refresh(ctx context.Context, acct *Account) (*Credential, error) // 账号型才实现
    Capabilities() Caps
}
```

> **网关面不放进 `.api` 文件**：go-zero 的 api 描述不适合表达流式透传和三协议路由，硬塞进去会生成一堆没用的类型。管理面用 `.api` + goctl 生成，网关面手写 handler 更清晰。

---

## 8. 里程碑（自用版）

> 本节已被 [migration-plan.md](./migration-plan.md) 第 6 节取代——因参考项目已完成三协议与调度，
> 原 M3/M4/M5 的技术工作大幅减少。以下仅保留供对照。

| 阶段 | 交付 | 验收 |
| --- | --- | --- |
| M1 骨架 | 四组导航、全部页面壳、空/加载/错误态 | 页面全可达，响应式正常 |
| M2 单协议打通 | `custom` 渠道 CRUD + 账号 CRUD + `/v1/chat/completions` 流式 + usage 落库 | curl 走通一次流式，记录页能看到 |
| M3 调度 | 选号、重试、换号、并发闸门、熔断、`/status` 面板 | 手动禁用账号，请求自动转移 |
| M4 三协议 | Anthropic Messages + Responses 入口 + 协议互转 | 三种客户端各跑通一次工具调用 |
| M5 账号型渠道 | workbuddy / trae 适配器接入（依赖你提供的反代代码） | 登录态过期能自动刷新，探活进状态页 |
| M6 观测 | 倍率计费、统计、告警 Webhook | 掉号 1 分钟内收到通知 |

---

## 9. 已确认的信息

1. **参考项目技术栈**：Tauri 2 + Rust，`src-tauri/src/gateway/` 下 21,044 行生产代码，三协议入口与调度已实现 → 见 [migration-plan.md](./migration-plan.md)。
2. **凭据来源**：纯 HTTP 请求 + Cookie/Token 持久化，**不依赖 WebView 登录**，可原样搬到服务端。
3. **向量模型**：不需要 `/v1/embeddings`，不排期。
4. **运营任务**：随网关一起迁移，提升为独立一级菜单 → 见 [task-module.md](./task-module.md)。
5. **视觉参考**：FastGPT（`E:\FastGPT-main`），只取 design token，不改布局 → 见 [visual-reference.md](./visual-reference.md)。
6. **Key 渠道隔离**：要做，`channel_allow` + `model_allow` 两个维度 → 见 [key-isolation.md](./key-isolation.md)。
7. **任务记录保留天数**：可配置，默认 90 天；**定时巡检**可配置，**默认开启**。

### 仍需确认

- 数据归属方案（PG 单一真相 / Rust 独占 / 混合），见迁移方案第 2 节第 3 项。
- 旧 `api_keys` 迁移策略、Key 级限速与并发是否同期做，见 [key-isolation.md](./key-isolation.md) 第 9 节。

