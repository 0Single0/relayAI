# 任务模块设计（渠道运营任务）

> 状态：**设计稿**，未落代码。前置：[product-plan.md](./product-plan.md)、[migration-plan.md](./migration-plan.md)。
> 来源：参考项目 `src-tauri/src/gateway/channels/workbuddy/{tasks,daily}.rs` + `commands/mod.rs` 的任务命令组。

---

## 0. 一句话

把「任务执行」从账号行的展开面板，提升为**独立一级菜单 + 按渠道分的二级菜单**，支持渠道级一键执行、指定账号执行、实时看每个账号的逐条进度。

---

## 1. 为什么不能只搬现有的

参考项目里任务能力的真实形态是**挂在账号上的**：

- 数据：`TaskInfo` / `DailyAction` 都是 `ChannelAdapter` 按 **单个账号** 取的（`list_tasks(client, account)`）
- 执行：`gateway_run_tasks(channel_id, uid)` —— 一次只跑一个账号
- 进度：`state.task_runs: HashMap<"channel/uid", Value>` —— 内存，只存"最近一次"

要支撑渠道级批量，有四处**必须重写**（不是改改参数就行）：

| 现状 | 问题 | 结论 |
| --- | --- | --- |
| `task_runs` 用 `HashMap` 存进度，**从不删除** | 每个账号永久占一条，账号多了就是内存泄漏 | 改 PG 表 + 内存只留"运行中" |
| 只有"最近一次"结果 | 重启全丢；看不到"昨天签到成功没有" | 加历史表 |
| 无取消 | 一个渠道 20 个账号、每个 20 项任务、每项最多 10 轮 × 1.1s ≈ **几分钟到几十分钟**，用户点错了只能等 | 加取消（任务间检查） |
| 只能单账号 | 渠道级一键需要"批次"概念 | 加 `batch_id` |

> 顺带一个真问题：`begin_task_run` / `finish_task_run` 的 key 是 `format!("{channel_id}/{uid}")`，永远不 `remove`。这是**泄漏**，不是"保留最近结果"的自觉设计——因为读的时候只读最新那一条，旧条目纯占内存。

---

## 2. 能力模型：菜单由能力决定，不由渠道硬编码

不同渠道的运营能力差别很大，必须在**菜单生成阶段**就知道，而且**不能为此打上游**：

| 渠道实例 | 任务清单 | 每日动作 | 单次签到 |
| --- | --- | --- | --- |
| WorkBuddy 国内版 `wb-cn` | ✅ | ✅ 签到 + 猫猫旅行 | ✅ |
| WorkBuddy 国际版 `wb-intl` | ❌（`realm != Cn` 直接 `Ok(None)`） | ✅ 每日活跃对话 | ✅ |
| Trae `trae-cn` | ❌ | ❌ | ✅ |
| 自定义供应商 `custom` | ❌ | ❌ | ❌ |

### 2.1 新增一个纯函数 hook

```rust
/// 该渠道有哪些运营能力。**不发网络请求**，只按配置/内部状态判断。
pub struct OpsCaps {
    pub tasks: bool,   // 有任务清单（可接取/上报/领奖）
    pub daily: bool,   // 有每日动作清单
    pub checkin: bool, // 有单次签到（没有清单）
}

trait ChannelAdapter {
    fn ops_caps(&self) -> OpsCaps { OpsCaps::default() }  // 默认全 false
}
```

各适配器实现：

- `workbuddy` CN：`tasks: true, daily: true, checkin: true`
- `workbuddy` Intl：`tasks: false, daily: true, checkin: true`（`checkin` 待实测，见 §8.1④）
- `trae`：`checkin: true`，其余 false
- `custom` / `fake`：全 false

**为什么不用配置里写死**：realm 是渠道 options 里的字段，但"国内版有任务、国际版没有"是适配器知识。写进配置会让两处真相打架，且新增渠道时要人去填。

### 2.2 菜单从接口来

`GET /api/v1/tasks/channels` 返回有能力的渠道：

```json
[
  { "id": "wb-cn",   "name": "WorkBuddy 国内版", "adapter": "workbuddy",
    "caps": { "tasks": true, "daily": true, "checkin": true },
    "accounts": 3, "today_pending": 2 },
  { "id": "wb-intl", "name": "WorkBuddy 国际版", "adapter": "workbuddy",
    "caps": { "tasks": false, "daily": true, "checkin": true },
    "accounts": 1, "today_pending": 0 }
]
```

> `caps` 全 false 的渠道**不出现在菜单里**——避免点进去一片空白。
> 但如果用户把某个渠道的所有账号都停了，不要隐藏菜单（否则会以为渠道没了），显示空状态即可。

---

## 3. 数据模型（PG）

任务执行记录属于**运行态但需持久**，和 `usage_logs` 一样由 Rust 直写 PG。

```sql
-- 一次执行：一个账号 × 一类动作
CREATE TABLE task_runs (
    id            BIGSERIAL PRIMARY KEY,
    batch_id      UUID,                      -- 渠道级一键执行时，N 个 run 共享
    channel_id    TEXT NOT NULL,
    account_uid   TEXT NOT NULL,
    kind          TEXT NOT NULL,             -- all | daily | tasks | checkin（与接口 scope 同值）
    status        TEXT NOT NULL,             -- running | done | partial | failed | cancelled
    trigger       TEXT NOT NULL,             -- manual | schedule
    total         INT  NOT NULL DEFAULT 0,   -- 计划执行项数
    done_count    INT  NOT NULL DEFAULT 0,
    failed_count  INT  NOT NULL DEFAULT 0,
    skipped_count INT  NOT NULL DEFAULT 0,
    earned        INT  NOT NULL DEFAULT 0,   -- 本轮拿到的积分
    error         TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ
);

CREATE INDEX task_runs_recent  ON task_runs (started_at DESC);
CREATE INDEX task_runs_account ON task_runs (channel_id, account_uid, started_at DESC);
CREATE INDEX task_runs_running ON task_runs (status) WHERE status = 'running';

-- 一条任务/动作的结果
CREATE TABLE task_run_items (
    id          BIGSERIAL PRIMARY KEY,
    run_id      BIGINT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    code        TEXT NOT NULL,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL,   -- done | skipped | failed | not_in_window
    message     TEXT NOT NULL DEFAULT '',
    earned      INT  NOT NULL DEFAULT 0,
    finished_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX task_run_items_run ON task_run_items (run_id, id);
```

### 3.1 几个刻意的决定

- **`task_run_items` 直接引用 `run_id`**：任务结果是"这一次"的，不是"这个任务"的。想做"最近 7 天某任务成功了几次"用 join 查，不另建聚合表。
- **`partial index` 只索引 `running`**：查"现在有哪些在跑"是热路径（每次进页面都查），表再大也只扫几十行。
- **不存 `TaskInfo` 清单**：清单是上游实时状态（进度会变），落库必然过期。页面每次进都现拉，只把**执行结果**存下来。
- **`kind` 四值而非三类**：`all`（一键，含每日动作+任务）、`daily`（每日动作）、`tasks`（任务清单）、`checkin`（单次签到）。`tasks` 与 `daily` 语义不同（一次性 vs 每天），放一起会重演参考项目"签完显示已完成、明天还得签"的混淆；但页面上用户只看到一个「一键执行」，落库时记 `all` 才还原得出"这次到底做了什么"。

---

## 4. 接口

### 4.1 读（Go 直连 PG）

```
GET /api/v1/tasks/channels                 # 有能力 + 账号数 + 今日待办数
GET /api/v1/tasks/channels/:id/accounts    # 该渠道账号 + 每账号今日状态摘要
GET /api/v1/tasks/runs?channel=&account=&status=&page=   # 历史
GET /api/v1/tasks/runs/:id                 # 单次详情（含 items）
```

### 4.2 实时快照（Rust，必须贴适配器）

```
GET  /api/v1/tasks/channels/:id/live        # 代理到 Rust：账号 × 任务清单 × 今日状态
```

返回结构（**关键：按账号分组，因为同一渠道不同账号清单可能不同**）：

```json
{
  "channel_id": "wb-cn",
  "fetched_at": "2026-10-21T16:30:00+08:00",
  "accounts": [
    {
      "uid": "u_abc",
      "nickname": "主号",
      "enabled": true,
      "credential_ok": true,
      "running": false,
      "run_id": 12,
      "tasks": [
        { "code": "create_canvas", "name": "创建设计画布", "status": "claimable",
          "progress": "1/1", "reward": 20, "automatable": true, "note": "" },
        { "code": "RichMeow_Chat", "name": "桌面对话事件链", "status": "not_accepted",
          "progress": "0/1", "reward": 10, "automatable": false,
          "note": "需在桌面端发起 1 次对话（伪造事件不计进度）" }
      ],
      "daily": [
        { "code": "sign_in", "name": "每日签到", "action": "签到",
          "done_today": false, "automatable": true, "note": "连续 3 天" },
        { "code": "cat_travel", "name": "猫猫旅行", "action": "派出旅行",
          "done_today": false, "automatable": true, "note": "旅行途中，稍后再来" }
      ]
    }
  ]
}
```

> `live` 是**N 次上游请求**（每账号一次 tasks + 一次 daily）。渠道账号多时会慢，必须：
> 并发限 3，总超时 15s，超时的账号返回 `status: "timeout"` 而不是让整个页面挂掉。

### 4.3 写（Go 校验参数 → 代理 Rust）

```
POST /api/v1/tasks/runs
  body: { "channel_id": "wb-cn", "account_uids": ["u_abc"], "scope": "all" }
  → { "batch_id": "…", "runs": [{ "run_id": 12, "account_uid": "u_abc" }] }

POST /api/v1/tasks/runs/:id/cancel
POST /api/v1/tasks/claim     { channel_id, account_uid, code }   # 单独领取（不上报）
```

`scope` 的取值：

| 值 | 含义 | 用在哪 |
| --- | --- | --- |
| `all`（默认） | 每日动作 → 任务清单（按 §8.1 的顺序与去重规则） | 页面上的「一键执行」 |
| `daily` | 只跑每日动作 | Trae 的任务页（只有签到） |
| `tasks` | 只跑任务清单 | 「只跑任务」的高级入口 |
| `checkin` | 只打一次签到 hook | 单独的签到按钮 |

> 页面上的主按钮一律发 `all`。`daily` / `tasks` / `checkin` 只用于窄场景，避免语义分裂成四个按钮。

### 4.4 进度推送

两个选择：

| 方式 | 优点 | 缺点 |
| --- | --- | --- |
| **SSE** `GET /tasks/runs/:id/stream` | 逐条实时，前端不用轮询 | 要处理断线重连、多标签页 |
| 轮询 `GET /tasks/runs/:id` 每 1s | 简单，和现有实现一致 | 慢任务要轮询几分钟 |

**推荐 SSE**：relayAI 是 Web 应用（不再是 Tauri 桌面端），SSE 天然可用，而且 `openai.rs` 里已有 SSE 编码的经验。断线重连按 `Last-Event-ID` 补发已完成的 items 即可（items 有自增 id）。

---

## 5. 页面设计

### 5.1 导航

```
任务
├─ 总览        /tasks
└─ <渠道名>    /tasks/:channelId        ← 每个有能力的渠道一项（数据驱动）
```

### 5.2 `/tasks` 总览

不重复渠道详情，只回答三个问题：

1. **今天还有谁的活没干？** —— 按渠道聚合的"待办账号数"
2. **现在有什么在跑？** —— 运行中的批次 + 进度条
3. **最近失败的是什么？** —— 最近 10 条 `failed` 的 run，点进去看 items

卡片式布局，每个有能力的渠道一张卡：渠道名、账号数、今日待办 X/Y、一键执行按钮。

### 5.3 `/tasks/:channelId` 渠道任务页

主体是一张**账号表**，不是任务表：

| 账号 | 今日进度 | 状态 | 最近执行 | 操作 |
| --- | --- | --- | --- | --- |
| 主号 | 3/8 项 · 签到未做 | 空闲 | 2 小时前 · 3 成功 | 一键执行 / 展开 |
| 备用号 | 0/8 项 | **执行中 2/8** | — | 取消 / 展开 |

- 顶部：`一键执行全部账号`（谨慎操作，见 §6.2）、`刷新`、筛选（只看待办/只看失败）
- 行展开 → 该账号的**每日动作**在上、**任务清单**在下（与执行顺序一致），逐条显示进度、奖励、不可自动化原因，以及本轮的实时结果
- 展开区里每行有独立按钮：`领取`（claimable）/ `执行`（daily 单项）

> **不把 20 个账号的任务一次全展开**：默认只展开一个。否则一个渠道 3 个账号 × 25 项 = 75 行，页面失控。

### 5.4 从账号页跳转

账号详情页保留一个链接：`查看该账号的任务 →` 跳到 `/tasks/:channelId?account=uid`（自动展开该行）。
**账号列表页不再内嵌任务面板**——按你的要求移除了。

### 5.5 Trae 这类"只有签到"的渠道

Trae 的 `daily_actions` 走默认实现返回 `Ok(None)`，只有 `sign_in` 被覆盖成真实的签到请求。所以页面退化成极简形态——没有任务区、没有每日动作区，只有一张账号表：

| 账号 | 今日签到 | 最近执行 | 操作 |
| --- | --- | --- | --- |
| 主号 | 已签 | 今天 09:12 | 再签一次（幂等） |
| 备用号 | **未签** | — | 签到 |

- 顶部按钮文案是「全部签到」，不是「一键执行」——**别让文案撒谎**。
- `scope` 发 `checkin`（不是 `daily`）：Trae 没有每日动作清单，`daily` 会跑出空 run。

---

## 6. 执行语义（最容易出事的部分）

### 6.1 并发与间隔

参考项目的间隔是给**定时任务**用的（`ops.rs`：账号间 800ms、每日动作间 1200ms）。手动批量执行要沿用同一套常量，否则会重演"整点齐发被当成异常流量"。

```
渠道级一键执行：
  账号串行（不是并发）
  账号之间 sleep 800ms（PER_ACCOUNT_GAP）
  账号内动作之间 sleep 1200ms（DAILY_GAP）
  单个账号超时 180s
  整批超时 30min（20 账号 × 25 项的上限量级）
```

**为什么账号必须串行**：同一出口 IP 并发打同一上游的运营接口，是最容易被风控识别的模式。

### 6.2 「一键执行全部账号」要有二次确认

这是**会真实消耗上游额度、且可能触发风控**的操作。确认框里必须显示：

- 本次将执行的账号数、预计项数（每日动作 + 任务项，**去重后**，见 §8.1②）
- 预计耗时（按 1.1s/项 × 项数 × 账号数估）
- 一句话风险提示

### 6.3 幂等

- **同一账号同一时刻只允许一个 run**：沿用 `begin_task_run` 的语义，但改成查 PG 的 `status='running'`（内存版重启就失效）。
- **`claim` 本身幂等**：上游 `already_claimed: true` 时返回 `done / 0 分`，不报错。
- **签到幂等**：上游 `code == 10001`（已签）按成功处理。
- **一键执行里每日动作只跑一次**：走 `daily_actions` 清单，不再叠加 `sign_in` hook（§8.1②）。

### 6.4 状态判定：`skipped` 不等于失败

`TaskRunResult.status` 已有五个值，迁移后**原样保留**，并且 run 的汇总状态要按它算：

| item status | 含义 | 计入 run 的 |
| --- | --- | --- |
| `done` | 成功（含"本来就已完成"） | `done_count` |
| `skipped` | 主动跳过（不可自动化 / 旅行途中 / 已达日限） | `skipped_count` |
| `not_in_window` | 不在时间窗（如 `black_cat` 只在 23:00–08:00） | `skipped_count` |
| `failed` | 真失败 | `failed_count` |

汇总规则：

```
failed_count == 0 && done_count > 0        → done
failed_count > 0 && done_count > 0         → partial
failed_count > 0 && done_count == 0        → failed
done_count == 0 && failed_count == 0       → done（全是 skipped，今天没什么可做）
```

**`skipped` 绝不能算进 `failed_count`**。否则 WorkBuddy 那种"一半任务需要桌面端手动完成"的渠道，每次一键执行都是 `partial`，用户很快就无视这个红点了。

### 6.5 取消

每个任务项之间检查取消标志（不能打断正在飞的上游请求，但能立刻停止后续项）：

```rust
for item in items {
    if cancel.load(Ordering::Relaxed) {
        run.status = "cancelled"; break;
    }
    ...
}
```

取消后已经产生的结果**保留**（`done` 的项不能回滚——积分已经到账了）。取消标志放内存（`HashMap<run_id, Arc<AtomicBool>>`），进程重启后"运行中"的 run 统一标 `failed`（error: "进程重启，执行中断"）。

### 6.6 定时执行

参考项目 `ops.rs` 已有：启动 20s 后巡检，每 60s 一轮，每日动作 09:00 / 21:00 各补一次。

迁移后**保留这个定时任务**，但要改成：
- 产生的 run 标 `trigger: "schedule"`，页面上和手动的区分开
- 失败不再默默重试，写入 run 记录让页面看得见

---

## 7. 要新写的部分 vs 直接搬的

| 能力 | 现状 | 动作 |
| --- | --- | --- |
| `tasks.rs` / `daily.rs` 上游协议 | 完整 | **直接搬**（约 1,066 行） |
| `TaskInfo` / `DailyAction` / `TaskRunResult` 结构 | 完整 | 直接搬 |
| 逐条进度推送（mpsc） | 完整 | 改写成写 PG + SSE |
| 单账号一键执行 | 完整 | 改造为 run 记录 |
| `task_runs` 内存 Map | 有泄漏 | **重写为 PG** |
| 渠道级批量 | 无 | **新写**（批次 + 串行调度） |
| 取消 | 无 | **新写** |
| 历史查询 | 无 | **新写**（Go 侧 SQL） |
| `ops_caps()` 能力探测 | 无 | **新写**（4 个适配器各几行） |
| 定时巡检 | 完整（`ops.rs`） | 直接搬，加 `trigger` 标记 |
| 每日动作清单 + 单项执行 | 完整（`daily.rs`） | 直接搬，加进 `all` 的执行序列 |
| 汇总状态的 `skipped` 语义 | 无（只存"最近一次"） | **新写**（见 §6.4） |

---

## 8. 已确认的决策

1. **Trae 也给一个渠道任务页**（只有一行「签到」）。
   Trae 没有任务清单也没有每日动作，但签到是每天要点的；藏在账号详情里会被忘掉。页面按能力渲染：`caps.tasks=false` → 不显示任务区，只显示「签到」一行。
2. **渠道级「一键执行」包含每日动作**。
   语义明确为：**把这个号今天该干的都干了**——先每日动作（签到、猫猫旅行、每日对话），再任务清单。

### 8.1 「包含每日动作」带来的五个必须处理的细节

**① 顺序：每日动作在前，任务在后。**

每日动作是单次请求、且时效性强（签到跨零点就漏了）；任务清单是长任务（每项最多 10 轮 × 1.1s）。先跑短的，用户能更早看到"今天签到成功了"，长任务慢慢跑。

**② 去重：`wb-cn` 的签到既是 daily action，也是 checkin hook，打的是同一个上游端点。**

```rust
// workbuddy/daily.rs
const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";
// workbuddy/auth.rs
pub const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";   // 同一个
```

一键执行**只能走 `daily_actions` 清单**，不能额外再调一次 `sign_in` hook。否则同一次执行会打两次签到接口——上游虽然幂等（`10001` 已签按成功），但会白白拉长风控窗口。

> `sign_in` hook 的用途应限定为「单独的签到按钮」和「登录成功后补签一次」，不进批量流程。

**③ 猫猫旅行是状态机，不能当普通任务"执行一次就算完"。**

| 上游 state | 该做什么 | 一键执行时的结果 |
| --- | --- | --- |
| `arrived` | 领奖 | `done` + 奖励 |
| `idle`（未达日限） | 派出（**必须带 `location_id`**，空 body 会 400） | `done` |
| `idle` + `daily_limit_reached` | 什么都不做 | `done`，message「今日已完成旅行」 |
| `traveling` | 等 | **`skipped`**，message「旅行途中，稍后再来」——**不是失败** |

关键：`traveling` 在一键执行里绝不能算 `failed`。否则每次批量都报一堆假失败，用户就再也不看这个页面了。

**④ `wb-intl` 的签到不在每日动作清单里。**

`daily_actions` 对国际版只返回 `daily_chat` 一项（`daily.rs`：`Realm::Intl => vec![chat_action(account)]`），但 `sign_in` hook 对两个 realm 都可用。所以国际版的「签到」只能通过 `caps.checkin` 单独呈现，不进 `daily` 数组。

> **待实测**：国际版 `www.workbuddy.ai` 是否真有签到接口。如果没有，`ops_caps()` 里 Intl 的 `checkin` 应置 false——别给用户一个点了必失败的按钮。

**⑤ 每日动作的 `done_today` 只用于展示，不用于跳过。**

`sign_in` 幂等（`10001`）、`cat_travel` 状态机自带判定，所以"今天已经做过"的动作**照样可以纳入执行序列**——让渠道自己返回 `done / 已做过`。反过来，如果在网关层用 `done_today` 提前跳过，就会出现"页面显示已签、实际上游没签"（`checkin-activity-status` 查询失败时那段代码就是按"没签"兜底处理的）。

例外：`traveling` 状态要跳过（见 ③），因为重复 depart 会 400。

### 8.2 已确认

1. **历史保留天数可配置。** 设置页给一个「任务记录保留天数」，默认 90 天，范围 7–365。清理任务按天跑一次，删 `task_runs`（`task_run_items` 随 FK 级联）。
2. **定时巡检保留，默认开启。** 沿用参考项目口径：启动 20s 后首轮，之后每 60s 巡检；每日动作在 09:00 / 21:00 各补一次失败项。设置页可关闭。

> 定时巡检默认开启的理由：自用的核心价值是"不用记着去点"。但必须有可见性——每次定时执行都写 `task_runs`（`trigger='schedule'`），失败在总览页红点提示。参考项目现在是"失败只写 stderr"，等于没有。

### 8.3 定时巡检与 Key 隔离的交互（新增约束）

定时巡检**不吃任何 API Key**——它是网关自己发起的运营动作，不经过 `/v1/*` 入口，也不计入 `usage_logs`。

但它**消耗的是同一批账号的上游额度**。所以：
- 巡检冲高时，交互式请求可能因为账号并发占满而 503。
- 账号并发上限（`per_account`，默认 3）要留给巡检 1 个槽位，或在巡检期间临时降为 1 并发（`traveling` 类的长轮询更明显）。

**建议**：巡检与交互式请求共用 `try_acquire()`，不预留槽位——让 503 自然发生。因为预留槽位意味着"哪怕没人用，也有一个并发永远空着"，对自用场景更亏。
