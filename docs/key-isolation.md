# API Key 的渠道隔离设计

> 状态：**设计稿**，未落代码。前置：[product-plan.md](./product-plan.md)、[migration-plan.md](./migration-plan.md)。
> 回答的问题：「我这个 Key 只能走 WorkBuddy，那个 Key 只能走 Trae」——要不要做，怎么做。

---

## 0. 结论

**要做，而且这是必须做的一项。** 现在的实现（`api_keys: Vec<String>`）只是"一串密钥白名单"，所有 Key 共享同一张模型矩阵和同一个号池，没有任何隔离。

但对本项目来说，隔离的动机**不是权限管理，而是成本归因和爆炸半径**——WorkBuddy 和 Trae 是你自己买的订阅额度，一个跑 Cursor 的 Key 顺着矩阵落到 Trae 上烧额度，你事后只能从用量记录里倒推。

---

## 1. 现状：三处都"糅合"在一起

| 位置 | 现在的行为 | 问题 |
| --- | --- | --- |
| `config.rs`：`api_keys: Vec<String>` | 纯字符串数组，无任何元数据 | 一个 Key 无法携带策略 |
| `server/openai.rs::authorize` | 常量时间比较，通过即放行 | 只有"有效/无效"两态 |
| `server/openai.rs::api_key_scope` | **只用于亲和作用域**（`short_hash`） | 不影响路由，不影响模型清单 |
| `core/matrix.rs`：`Matrix` | 全局一张，所有渠道目录合并 | 同名模型被合并成一条多路由的候选 |
| `core/dispatch.rs::candidates` | 按模型取全部路由 × 全部账号 | 不看 Key |

关键一点：**同名模型会被合并**。`wb-cn` 和 `trae-cn` 都提供 `deepseek-v4.1-flash` 时，矩阵里它是**一条**对外模型、**两条**路由。所以"按渠道隔离"不能靠模型名区分，必须按路由的 `channel_id` 过滤。

---

## 2. 隔离的维度：两个正交轴

```
Key  ──┬── channel_allow[]  ：这个 Key 能用哪些渠道（空 = 不限）
       └── model_allow[]    ：这个 Key 能用哪些对外模型（空 = 不限）
```

两个都做，因为它们回答不同的问题：

- `channel_allow`：**别把我的 Trae 额度烧了**（成本边界）
- `model_allow`：**别让脚本乱调贵的模型**（质量/费用边界）

只有 `model_allow` 是不够的：同名模型跨渠道合并，限制不了走哪个渠道。
只有 `channel_allow` 也不够：同一个渠道路由下可能挂着几倍价差的模型。

### 2.1 数据模型

```sql
ALTER TABLE api_keys
    ADD COLUMN channel_allow TEXT[],   -- 渠道实例 id 列表，空数组 = 不限
    ADD COLUMN model_allow   TEXT[];   -- 对外模型名列表，空数组 = 不限
```

> **存显式渠道 id，不存"适配器类型"。** `wb-cn` / `wb-intl` 是两个渠道实例，用户说"WorkBuddy 的模型"时通常两个都要，但"Trae 企业版"和"Trae 个人版"可能只想给一个。
> 界面上提供**按适配器快选**（点一下"WorkBuddy"自动勾上 `wb-cn` + `wb-intl`），但落库存的是展开后的 id。

**这样做的代价要写清楚**：以后新增一个 `wb-jp` 渠道时，已经绑定过的 Key 不会自动包含它。对隔离场景来说这是**正确**的（显式优于隐式），但界面上要在 Key 详情里提示"当前渠道清单是创建时的快照"。

### 2.2 不做"渠道组"表

`sub2api` 那类项目有"用户组 → 渠道组"的多对多。自用场景下这套是过度设计：Key 就那么几个，直接列表即可。等真的需要"十几个 Key 共享一套渠道策略"时再加，那时也只是加一张 `channel_groups` + 反查，不影响现在的表结构。

---

## 3. 生效点：三处必须同时改，漏一处就漏

### 3.1 鉴权 → 解析出完整的 Key 上下文

现在 `authorize` 返回 `Result<(), GatewayError>`，需要改成返回 Key 实体：

```rust
pub struct KeyContext {
    pub key_id: i64,
    pub name: String,
    pub channel_allow: Vec<String>,   // 空 = 不限
    pub model_allow: Vec<String>,     // 空 = 不限
}

pub(super) fn authorize(state: &SharedGateway, headers: &HeaderMap)
    -> Result<KeyContext, GatewayError>;
```

### 3.2 模型清单：`/v1/models` 要跟着过滤

**这是最容易漏的一处。** 不过滤的后果：客户端（Cursor / Cherry Studio）看到 20 个模型，实际只有 8 个能用，用户会以为是平台坏了。

```rust
// list_models 里
matrix.public_models()
    .into_iter()
    .filter(|model| key.allows_model(model))
    .filter(|model| matrix.routes(model).iter().any(|r| key.allows_channel(&r.channel_id)))
    .collect()
```

注意第二个 filter 是 `any` 不是 `all`：一个模型只要**还有一条**本 Key 能走的路由，就该出现在清单里。

### 3.3 调度：候选过滤在建候选集时做，不在选号后做

```rust
// core/dispatch.rs::open_chat_session
let requirements = Requirements::from_request(request, estimate_request_tokens(request));
let mut set = router::candidates(&matrix, &requirements, &state.pool, &request.model);

// 新增：Key 权限过滤（在选号之前，而不是选完之后再检查）
set = set.restrict_to(&key.channel_allow, &key.model_allow);
```

**为什么必须在建候选集时做**：如果选完号再检查"这个号属于允许的渠道吗"，就会出现"选到 → 拒绝 → 重试 → 再选到同一个"的死循环，而且并发槽已经被占过一轮。过滤要发生在打分之前。

### 3.4 亲和要跟着走

`AffinityStore::candidate_keys(scope, ...)` 里的 `scope` 现在已经是 `short_hash(key)`，所以不同 Key 的绑定天然隔离——**这一点现状是对的，不用改**。

但要补一条：Key 的 `channel_allow` 变更后，该 Key 的旧亲和绑定可能指向一个已经不被允许的渠道。要么在改 Key 时清掉它的绑定，要么在亲和命中后加一次 `allows_channel` 校验。**选后者**（省一次全表扫），并在命中失败时按现有"绑定失效"路径清掉。

---

## 4. 错误语义：三种情况必须能分清

现有 `dispatch.rs` 已经把"请求要求不满足"（400）和"没有可用账号"（503）分开了。Key 权限是**第三种**，不能混进前两者：

| 情况 | 应该返回 | 错误类型 | 为什么 |
| --- | --- | --- | --- |
| 模型名写错 / 没有任何渠道提供 | 400 `invalid_request_error` | `InvalidRequest` | 调用方问题 |
| 模型存在，但**这个 Key 的渠道都不提供它** | **403** `permission_error` | 新增 `KeyForbidden` | 权限问题，重试无用 |
| 模型存在、Key 也有权限，但账号全在冷却 | 503 `rate_limit_error` | `NoAccount` | 容量问题，重试有用 |

```rust
// 新增错误类型
ErrorKind::KeyForbidden  // → HTTP 403
```

消息必须说清是哪一维被挡了：

```
该 Key 无权使用模型 deepseek-v4.1-flash：允许的渠道（trae-cn）都不提供这个模型
该 Key 无权使用渠道 wb-cn：允许的渠道是 trae-cn
```

> 这条不只是"友好提示"。如果 403 被报成 503，用户会一直去查账号冷却、查上游状态，而真正的原因在 Key 配置里——**这就是最贵的那种排障时间**。

---

## 5. 用量记录：Key 是成本归因的主键

`usage_logs` 目前只有 `key_id`（在 product-plan 的表里已预留），但要保证它**真的被写入**：

```sql
key_id  BIGINT  -- 每个请求都写，鉴权阶段就能拿到
```

三条必须建的索引/视图：

```sql
CREATE INDEX usage_key ON usage_logs (key_id, ts DESC);

-- 按 Key × 渠道的日汇总（"这个 Key 今天烧了 Trae 多少额度"）
CREATE VIEW usage_daily_by_key_channel AS
SELECT key_id, channel_id, date_trunc('day', ts) AS day,
       count(*) AS requests,
       sum(prompt_tokens + completion_tokens) AS tokens
FROM usage_logs
WHERE status <> 'rejected'
GROUP BY 1, 2, 3;
```

这条视图直接支撑页面上的一个问题：**"我这个月 WorkBuddy 的额度是被谁用掉的"**。没有它，隔离就只是"防住"而没有"看清"。

---

## 6. 界面上怎么呈现

### 6.1 `/keys` 列表

| 名称 | Key | 渠道 | 模型 | 今日用量 | 最近使用 |
| --- | --- | --- | --- | --- | --- |
| Cursor 专用 | `sk-abcd****` | WorkBuddy（2） | 全部 | 1.2M tok | 3 分钟前 |
| 脚本 | `sk-efgh****` | Trae（1） | 3 个 | 80K tok | 昨天 |

「渠道」列显示"适配器名（数量）"而不是罗列 id——`wb-cn` 这种内部 id 对使用者没意义。

### 6.2 新建 / 编辑抽屉

- **渠道**：按适配器分组的多选（WorkBuddy / Trae / 自定义），组内可展开到具体渠道实例。默认「全部渠道」。
- **模型**：默认「全部模型」，但选项范围**要随渠道选择联动**——选了 Trae 就不该能勾一个只在 WorkBuddy 上的模型。
- 两个都留空 = 不限（保持现在的行为，避免升级后旧 Key 全部失效）。

### 6.3 空结果的兜底

如果某个 Key 的 `channel_allow` 指向的渠道被删了，页面上要显示成"1 个渠道已失效"而不是静默少一个。否则用户会以为是自己选错了。

---

## 7. 与"多 Key 分客户端"的关系

自用场景下 Key 的实际用途是**区分客户端**：Cursor 一个、脚本一个、Claude Code 一个。

隔离做好之后，这件事才真正成立：

- Cursor 的 Key 绑 `wb-cn`（走 IDE 场景，额度和行为都最匹配）
- Claude Code 的 Key 绑 `trae-cn`（Anthropic 协议入口 + Trae 的协议更贴）
- 脚本的 Key 绑全部（自动化测试用，需要跨渠道验证）

不做隔离的话，这三个 Key 只是三个"记账标签"，谁来都能烧光你所有渠道的额度。

---

## 8. 实施顺序

| 步骤 | 内容 | 依赖 |
| --- | --- | --- |
| 1 | `api_keys` 加两列 + `KeyContext` 解析 | 无 |
| 2 | `/v1/models` 过滤（§3.2） | 步骤 1 |
| 3 | 候选集过滤（§3.3）+ `KeyForbidden` 错误 | 步骤 1 |
| 4 | 亲和命中后校验（§3.4） | 步骤 3 |
| 5 | 前端 Key 编辑抽屉 + 渠道/模型联动 | 步骤 1 |
| 6 | `usage_daily_by_key_channel` 视图 + 页面 | 步骤 1 |

> 步骤 2 和 3 **必须一起上**。只做 3 会出现"清单里有、调用 403"；只做 2 会出现"清单里没有、但直接调用能通"——后者更糟，等于隔离没生效却让人以为生效了。

---

## 9. 待确认

1. **旧 Key 的迁移**：现在配置里的 `api_keys: Vec<String>` 没有元数据，迁移时一律落成"两个 allowlist 都为空 = 不限"，保持行为不变。确认吗？
2. **是否需要"Key 级限速"**（RPM / 日 Token 上限）一起做？表里已预留 `rpm_limit` / `daily_quota`，但**限速的执行点也在候选过滤那一层**，和本次改动同源，一起做更省事。
3. **是否需要"Key 级并发上限"**？如果某个 Key 要跑批量脚本，不限并发会把号池打满，拖慢交互式客户端。
