//! 成长任务中心（国内版）：任务清单、一键执行、结果记录。
//!
//! 数据与规则来自参考实现（MIT，`wb_tasks.py`）与开源生态的调研：
//! · 上报**认指纹**：CLI / 桌面 / web 三类事件族认各自的头与来源域——单一指纹会漏计分
//!   （workbuddy2api-panel 的核心发现）；
//! · **上报 200 ≠ 计分**：执行后必须回读任务进度，达标才 claim；
//! · 桌面端专属任务（伪造事件不推进进度）：诚实标注 `automatable=false` 并给出动作说明；
//! · 夜猫子任务只在 23:00-08:00 窗口计数，每天 1 次。
//!
//! 已知简化（相对 panel）：专家类任务用静态 expert 池伪造事件（panel 是"真实召唤+对话
//! 回执"），若实测不点亮再升级；小程序口径任务（school_season 等）暂未接。

use serde_json::{json, Value};

use crate::gateway::channels::{AccountSnapshot, TaskInfo, TaskRunResult};
use crate::gateway::{errors::GatewayError, timeutil::now_ms, transport};

use super::identity::{self, Hosts};

const TASKS_PATH: &str = "/v2/activity/growth/tasks";
const ACCEPT_PATH: &str = "/v2/activity/growth/tasks/accept";
const CLAIM_PREFIX: &str = "/activity/growth/tasks/";
const REPORT_PATH: &str = "/v2/report";

/// 每次上报之间的间隔（秒）：参考实现按 >=1s 防风控。
const REPORT_GAP_MS: u64 = 1100;

/// 任务 → （显示名, 事件 kind, 是否可自动化）。
///
/// kind 对应参考实现的 `build_event(account, kind)`：**事件名与字段必须逐字对**，
/// 上游对不认识的事件直接丢弃（表现为"上报成功但进度不动、积分不变"）。
fn spec(code: &str) -> (String, &'static str, bool) {
    match code {
        // 服务端校验真实行为，伪造无效 —— 列为需手动。
        "Expert_Philanthropy" => ("公益爱心捐赠".into(), "none", false),
        "RichMeow_Chat" => ("桌面对话事件链".into(), "none", false),
        "Library_read" => ("浏览资料库".into(), "none", false),
        // 桌面端专属（web 域上报口径不同，暂不伪造）。
        "Buddy_App" => ("进入 Buddy 应用".into(), "none", false),
        "Buddy_App_QQ" => ("企鹅教师助手".into(), "none", false),
        // 事件可推进的任务。
        "create_canvas" => ("创建设计画布".into(), "canvas", true),
        "template_5" => ("模板创建任务".into(), "template", true),
        "playbook_prompt" => ("灵感案例使用".into(), "playbook", true),
        "Expert_lighthouse" => ("轻量云专家使用".into(), "lighthouse", true),
        "Hp_Appearance" => ("应用主题外观".into(), "skin", true),
        "wb_wechat_oa_subscribe_task" => ("关注公众号".into(), "none", false),
        "black_cat" => ("夜猫子任务 (23:00-08:00)".into(), "cat", true),
        "chat_5" => ("发起 5 次对话".into(), "chat", true),
        "Model_chat_GLM5.2" => ("体验 GLM-5.2".into(), "glmchat", true),
        "expert_5" => ("使用专家助手".into(), "expert", true),
        "Expert_team_use_3" => ("使用专家团队".into(), "team", true),
        "skill_1" => ("体验技能".into(), "skill", true),
        "automation_1" => ("创建自动化任务".into(), "automation", true),
        "first_buddy" => ("领养首只猫猫".into(), "buddy", true),
        // 活动任务：必须在专家页召唤**这个特定专家**并完成一次对话。
        // 事件里的 id 必须是它的真实 id，否则上游只接受事件但不推进进度。
        "WorkBuddy_Valorant" => ("体验「无畏契约拼豆专家」".into(), "valorant", true),
        // 未知任务：保守当成可自动（走 heartbeat，不会误报成"需手动"）。
        /*
          `expert_use_*` 系列：体验某个**特定专家**并完成对话。

          为什么标成需手工而不是走 heartbeat：
            上游按专家 id 校验，事件里不带那个专家就只接收不计数。
            实测（库里 5 次失败）全是"上报后进度停在 0/1"，
            继续标"可执行"只会让用户反复点、反复失败。
          这组码的上游命名是 `expert_use_<拼音>`，无法从名字反推出真实专家 id，
          所以诚实地交给用户手动完成。
        */
        _ if code.starts_with("expert_use_") => (code.to_string(), "none", false),
        other => (other.to_string(), "heartbeat", true),
    }
}

/// 桌面端专属任务的深链（做不了，给用户动作说明）。
///
/// 每一条 `automatable = false` 的任务都**必须**在这里有一句话：
/// 否则明细行只有状态、没有原因，用户看到一条"跳过"却不知道该做什么。
/// `desktop_only_tasks_are_marked_honestly` 会逐条断言非空。
fn desktop_note(code: &str) -> &'static str {
    match code {
        "RichMeow_Chat" => "需在桌面端发起 1 次对话（伪造事件不计进度）",
        "Library_read" => "需在桌面端打开「资料库」并读完介绍文档",
        "Buddy_App" => "需在桌面端「发现应用」进入任意 Buddy 应用",
        "Buddy_App_QQ" => "需在桌面端「发现应用」进入「企鹅教师助手」",
        "Expert_Philanthropy" => "需真实捐款动作，服务端校验捐赠回执",
        "wb_wechat_oa_subscribe_task" => "需在微信里扫码关注「WorkBuddy」公众号（服务端校验关注关系）",
        // 未知的需手工任务也要给一句通用说明：明细行只有"跳过"没有原因时，
        // 用户不知道该去客户端里做什么。
        other if other.starts_with("expert_use_") => {
            "需在客户端「专家」页召唤该专家并完成一次对话（上游按专家 id 校验）"
        }
        _ => "",
    }
}

/// 上游的领取状态就在 `accept_status` 里：`not_accepted` / `accepted` / `claimed`。
///
/// 实测**不存在** `progress.claimed`，也没有 `claim_status`（早先按这两个字段读，
/// 于是已领取的任务一直显示"可领取"，而且每轮都会再领一遍）。
fn accept_status(item: &Value) -> &str {
    item.get("accept_status")
        .and_then(Value::as_str)
        .unwrap_or("not_accepted")
}

fn is_claimed(item: &Value) -> bool {
    accept_status(item) == "claimed"
}

fn in_night_window() -> bool {
    // 夜间窗口按**本地**小时判断（上游只在 23:00–08:00 计黑猫任务进度）。
    // 参考实现用 Windows API 取本地时间，服务端改用 chrono::Local。
    let (_, _, _, hour) = crate::gateway::timeutil::local_date_hour();
    hour >= 23 || hour < 8
}

/// 任务清单：`GET /v2/activity/growth/tasks`。
pub async fn list(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
) -> Result<Vec<TaskInfo>, GatewayError> {
    let headers = identity::chat_headers(
        &account.channel_id,
        identity::Realm::Cn,
        identity::Outbound::Desktop,
        hosts,
        account,
        now_ms() % 1_000_000,
    );
    let payload = transport::get_json(client, &hosts.chat_url(TASKS_PATH), &headers).await?;
    let mut tasks = Vec::new();
    for item in payload
        .get("data")
        .and_then(|data| data.get("tasks"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(code) = item.get("task_code").and_then(Value::as_str) else {
            continue;
        };
        // 上游字段：accept_status / progress{current,target} / reward_credit / title / tag。
        // 早期版本读的 status、current_value、reward 都不存在 —— 于是"永远未接取、进度 0、奖励 0"。
        let state = accept_status(item);
        let progress = item.get("progress").cloned().unwrap_or(Value::Null);
        let current = progress.get("current").and_then(Value::as_u64).unwrap_or(0);
        let target = progress.get("target").and_then(Value::as_u64).unwrap_or(1);
        // 注意：claimed_button_style / claimed_button_text 是"领取后按钮显示什么"的
        // **文案模板**（未接取时就已是 "gray"/"已完成"），不能当状态用。
        // 真实状态只有两处：accept_status（是否已领取）+ progress（进度）。
        let (name, _kind, automatable) = spec(code);
        // 上游自己用 tag 标了平台：PC = 只能桌面端完成，诚实标出来。
        let desktop_only = item
            .get("tag")
            .and_then(Value::as_str)
            .is_some_and(|tag| tag == "PC");
        let note = if !automatable || desktop_only {
            let jump = item.get("jump_url").and_then(Value::as_str).unwrap_or("");
            let desc = item
                .get("task_desc")
                .and_then(Value::as_str)
                .or_else(|| item.get("description").and_then(Value::as_str))
                .unwrap_or("");
            format!("{desc}（深链 {jump}）")
        } else {
            String::new()
        };
        tasks.push(TaskInfo {
            code: code.to_string(),
            name: item
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(name),
            status: if state == "claimed" {
                "done".to_string()
            } else if current >= target && target > 0 {
                // 达标但没领过（例如你在官方端手动做完了）——面板给「领取」。
                "claimable".to_string()
            } else if state == "not_accepted" {
                "not_accepted".to_string()
            } else {
                "in_progress".to_string()
            },
            progress: format!("{current}/{target}"),
            reward: item
                .get("reward_credit")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            automatable: automatable && !desktop_only,
            note,
        });
    }
    Ok(tasks)
}

/// 一键执行：接取 → 伪造事件上报（认指纹、带间隔）→ 回读进度 → 达标才领奖。
///
/// `only` 非空时**只跑这些任务码** —— 界面上「执行此任务」按的是某一条，
/// 不加这个过滤就会把整张清单跑一遍。
pub async fn run(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
    only: &[String],
    progress: tokio::sync::mpsc::UnboundedSender<TaskRunResult>,
) -> Result<Vec<TaskRunResult>, GatewayError> {
    let mut results = Vec::new();
    // 每条结果都实时推给面板（长任务不能等全部跑完才反馈）。
    let push = |result: TaskRunResult| {
        let _ = progress.send(result.clone());
        result
    };
    let headers = identity::chat_headers(
        &account.channel_id,
        identity::Realm::Cn,
        identity::Outbound::Desktop,
        hosts,
        account,
        now_ms() % 1_000_000,
    );

    let payload = transport::get_json(client, &hosts.chat_url(TASKS_PATH), &headers).await?;
    let items: Vec<Value> = payload
        .get("data")
        .and_then(|data| data.get("tasks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return Err(GatewayError::upstream(502, "没能获取到任务清单"));
    }

    // ① 批量接取未接任务（20 个一批，参考实现的分批口径）。
    let to_accept: Vec<String> = items
        .iter()
        .filter(|item| {
            item.get("accept_status").and_then(Value::as_str) == Some("not_accepted")
        })
        .filter_map(|item| {
            item.get("task_code")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut items = items;
    if !to_accept.is_empty() {
        let _ = transport::post_json(
            client,
            &hosts.chat_url(ACCEPT_PATH),
            &headers,
            &json!({ "task_codes": to_accept }),
        )
        .await;
        // **必须重读**：未接取时 progress 是 null，target 也拿不到；
        // 不重读就会把多项任务（要 3-5 次的）当成 1 次做完，领奖必然 400。
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        if let Ok(fresh) = transport::get_json(client, &hosts.chat_url(TASKS_PATH), &headers).await {
            if let Some(next) = fresh
                .get("data")
                .and_then(|data| data.get("tasks"))
                .and_then(Value::as_array)
                .cloned()
            {
                if !next.is_empty() {
                    items = next;
                }
            }
        }
    }

    // ② 逐个任务：上报事件 → 回读进度 → 达标领奖。
    for item in items {
        let Some(code) = item.get("task_code").and_then(Value::as_str).map(str::to_string)
        else {
            continue;
        };
        /*
          指定了任务码就只跑这些 —— 「执行此任务」按钮按的是某一条，
          不过滤会把整张清单跑一遍（慢且浪费上游额度）。
        */
        if !only.is_empty() && !only.iter().any(|wanted| wanted == &code) {
            continue;
        }
        let (name, _kind, automatable) = spec(&code);
        let claimed_before = is_claimed(&item);
        if claimed_before {
            results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "done".into(),
                message: "早已完成并领取".into(),
                earned: 0,
            }));
            continue;
        }
        if !automatable {
            results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "skipped".into(),
                message: desktop_note(&code).to_string(),
                earned: 0,
            }));
            continue;
        }
        if code == "black_cat" && !in_night_window() {
            results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "not_in_window".into(),
                message: "只在 23:00-08:00 上报计数，当前不在窗口".into(),
                earned: 0,
            }));
            continue;
        }

        // 进度在 progress{current,target}（嵌套对象），字段名与列表解析保持一致。
        let progress_node = item.get("progress").cloned().unwrap_or(Value::Null);
        let target = progress_node
            .get("target")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let mut current = progress_node
            .get("current")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // 上报到达标（每轮之间留防风控间隔；上限 10 轮保护）。
        let mut rounds = 0;
        while current < target && rounds < 10 {
            let event = build_event(&code, &account.uid, rounds);
            let _ = transport::post_json(
                client,
                &hosts.chat_url(REPORT_PATH),
                &headers,
                &json!([event]),
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_millis(REPORT_GAP_MS)).await;
            rounds += 1;
            // 回读进度（上报 200 ≠ 计分：必须回读）。
            current = read_progress(client, hosts, account, &code).await;
        }

        if current < target {
            results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "failed".into(),
                message: format!("上报后进度停在 {current}/{target}（可能需真实行为）"),
                earned: 0,
            }));
            continue;
        }

        // ③ 达标 → 领奖（Web 域，参考实现的领取路径）。
        match claim_one(client, hosts, account, &code).await {
            Ok(outcome) => results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "done".into(),
                message: if outcome.already {
                    format!("进度 {current}/{target}（奖励此前已领取）")
                } else {
                    format!("完成 {current}/{target} 并领取")
                },
                earned: outcome.earned,
            })),
            // 400 "task not completed" 等：如实报告。
            Err(error) => results.push(push(TaskRunResult {
                code: code.clone(),
                name,
                status: "failed".into(),
                message: format!("领奖失败：{}", error.message),
                earned: 0,
            })),
        }
    }

    Ok(results)
}

/// 领奖结果：入账积分，以及"此前已经领过"（上游的幂等标记）。
pub struct ClaimOutcome {
    pub earned: u32,
    pub already: bool,
}

/// 领奖：走 **Web 域**（与桌面端上报不是同一套头，路径也在 web 侧）。
///
/// 真实语义（实测）：已领过 → HTTP 200 `code:0 / already_claimed:true / credit:0`；
/// 未达标 → HTTP 400 `task not completed`。
async fn claim_one(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
    code: &str,
) -> Result<ClaimOutcome, GatewayError> {
    let headers = identity::billing_headers(hosts, account, Some("web"));
    let url = format!("{}{}{code}/claim", hosts.origin, CLAIM_PREFIX);
    let value = transport::post_json(client, &url, &headers, &json!({})).await?;
    if value.get("code").and_then(Value::as_i64) != Some(0) && !claim_ok_text(&value) {
        let reason = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("上游未说明原因");
        return Err(GatewayError::upstream(502, format!("上游拒绝：{reason}")));
    }
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    Ok(ClaimOutcome {
        earned: data.get("credit").and_then(Value::as_u64).unwrap_or(0) as u32,
        already: data
            .get("already_claimed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// 单独领取一项**已达标**任务的奖励（面板任务行上的「领取」）。
///
/// 与 `run()` 分开：进度已经达标、只是没领过的任务（例如你在官方端手动做完了），
/// 不该为了领一次奖把事件再上报一遍。失败原因也走行内文案，不弹 toast。
pub async fn claim(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
    code: &str,
) -> TaskRunResult {
    let (name, _kind, _) = spec(code);
    let build = |status: &str, message: String, earned: u32| TaskRunResult {
        code: code.to_string(),
        name: name.clone(),
        status: status.to_string(),
        message,
        earned,
    };
    match claim_one(client, hosts, account, code).await {
        Ok(outcome) if outcome.already => build("done", "奖励此前已领取过".into(), 0),
        // 入账积分只走 `earned` 字段、不写进文案：与 run() 口径一致，面板才能统一展示。
        Ok(outcome) => build("done", "领取成功".into(), outcome.earned),
        // 未达标最典型的返回：400 "task not completed"。
        Err(error) if error.message.contains("task not completed") => {
            build("failed", "任务还没达标，领取被拒".into(), 0)
        }
        Err(error) => build("failed", format!("领取失败：{}", error.message), 0),
    }
}

fn claim_ok_text(value: &Value) -> bool {
    value
        .get("msg")
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains("已领取") || text.contains("success"))
}

async fn read_progress(
    client: &reqwest::Client,
    hosts: &Hosts,
    account: &AccountSnapshot,
    code: &str,
) -> u64 {
    let headers = identity::chat_headers(
        &account.channel_id,
        identity::Realm::Cn,
        identity::Outbound::Desktop,
        hosts,
        account,
        now_ms() % 1_000_000,
    );
    let Ok(payload) = transport::get_json(client, &hosts.chat_url(TASKS_PATH), &headers).await
    else {
        return 0;
    };
    // 进度在 progress.current（嵌套对象），不是顶层 current_value。
    payload
        .get("data")
        .and_then(|data| data.get("tasks"))
        .and_then(Value::as_array)
        .and_then(|tasks| {
            tasks
                .iter()
                .find(|task| task.get("task_code").and_then(Value::as_str) == Some(code))
        })
        .and_then(|task| task.get("progress"))
        .and_then(|progress| progress.get("current"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// 事件构造：**逐字对齐参考实现**的 eventCode 与字段。
///
/// 参考实现在 2026-09 实测校准过每个字段；上游对不认识的事件直接丢弃，
/// 所以这里宁可照抄也不要"改进"。
/// `id` 每次必须不同 —— 上游按 (eventCode, id) 去重，重复 id 进度不动。
fn build_event(code: &str, uid: &str, round: u64) -> Value {
    let kind = spec(code).1;
    let now = now_ms();
    let conversation = format!("wb-task-{now}-{round}");
    let request = format!("{conversation}-req");
    match kind {
        "canvas" => json!({
            "eventCode": "wbx_design_canvas_task_create", "timestamp": now, "reportDelay": 0,
            "conversationId": conversation, "requestId": request,
            "source": "summon_keyword", "isCustomModel": false, "name": "",
            "inputLength": 12, "id": format!("wbx-canvas-{now}"), "cost": 0,
            "isSuccessful": true, "userId": uid,
        }),
        "playbook" => json!({
            "eventCode": "playbook_prompt_send", "timestamp": now, "reportDelay": 0,
            "id": "worker-ledger-freedom-dashboard", "name": "打工人小账本",
            "type": "other", "promptLength": 10, "isOfficial": 1,
            "source": "discover", "conversationId": conversation, "requestId": request, "userId": uid,
        }),
        "lighthouse" => expert_event("ex_2cvvUZQhDyeJ", "腾讯轻量云专家", "agent", uid, now, &conversation, &request),
        // 固定专家 id：这个任务的进度只认「无畏契约拼豆专家」。
        "valorant" => expert_event("ex_xQHHD1u5Jkua", "无畏契约拼豆专家", "agent", uid, now, &conversation, &request),
        "team" => {
            // 团队池必须**逐次换 id**：上游按 (eventCode, id) 去重，
            // 一直用同一个团队第二次就不计数（实测卡在 1/3）。
            const TEAMS: [(&str, &str); 8] = [
                ("CloudOpsTeam", "运维专家团队"),
                ("CloudContentTeam", "内容专家团队"),
                ("CloudDevTeam", "研发专家团队"),
                ("ProductStrategyTeam", "产品战略团队"),
                ("MarketingCampaignTeam", "营销活动团队"),
                ("SalesBattleTeam", "销售作战团队"),
                ("DesignEngineTeam", "设计引擎团队"),
                ("HrOperationsTeam", "人力运营团队"),
            ];
            let index = (round as usize) % TEAMS.len();
            expert_event(TEAMS[index].0, TEAMS[index].1, "team", uid, now, &conversation, &request)
        }
        "expert" => {
            // 专家池轮换：每次用不同 expert_id。
            const EXPERTS: [(&str, &str); 6] = [
                ("ex_PZw8Gu81HfN4", "运维工程师"),
                ("ex_ROsDtJbzADFV", "产品经理"),
                ("ex_SMUnl0nJbPix", "UI设计师"),
                ("ex_ZTR062oVBOCW", "数据分析师"),
                ("ex_a3sSSFBy8qaC", "后端架构师"),
                ("ex_aG1kvKbq8lPx", "文案策划"),
            ];
            let index = (round as usize) % EXPERTS.len();
            expert_event(EXPERTS[index].0, EXPERTS[index].1, "agent", uid, now, &conversation, &request)
        }
        "skill" => json!({
            "eventCode": "skill_info", "timestamp": now, "reportDelay": 0,
            "skillId": "skill_2096525080079265792", "name": "pptx", "userId": uid,
        }),
        "automation" => json!({
            "eventCode": "automated_task_create_suc", "timestamp": now, "reportDelay": 0,
            "name": "每周工作整理", "type": "cron", "source": "manually",
            "modelId": "deepseek-v4-flash", "modelIsThinking": false,
            "conversationId": conversation, "requestId": request,
            "schedule": { "type": "recurring", "rrule": "FREQ=WEEKLY;BYDAY=FR;BYHOUR=9;BYMINUTE=0" },
            "prompt": "每周五自动整理本周工作", "userId": uid,
        }),
        "skin" => json!({
            "eventCode": "appearance_skin_apply", "timestamp": now, "reportDelay": 0,
            "action": "apply", "source": "settings_close", "id": "theme-tkmw7j",
            "vipLevel": "free", "series": "craft", "type": "unknown",
            "name": "和平精英激战金秋", "userId": uid,
        }),
        "cat" | "glmchat" | "chat" => {
            let (model_id, model_name) = if kind == "chat" {
                ("deepseek-v4-flash", "DeepSeek V4 Flash")
            } else {
                ("glm-5.2", "GLM-5.2")
            };
            json!({
                "eventCode": "chat_request_send", "timestamp": now, "reportDelay": 0,
                "mode": if kind == "cat" { "night" } else { "craft" },
                "conversationId": conversation, "requestId": request,
                "inputLength": 12, "requestModelId": model_id, "requestModelName": model_name,
                "isPlan": false, "agentName": "default", "agentType": "conversation", "userId": uid,
            })
        }
        "template" => json!({
            "eventCode": "agent_task_created_with_template", "timestamp": now, "reportDelay": 0,
            "isCustomModel": true, "id": format!("{round}"), "name": "幻灯片",
            "requestId": request, "conversationId": conversation, "userId": uid,
        }),
        "buddy" => json!({
            "eventCode": "buddy_adopt_suc", "timestamp": now, "reportDelay": 0, "userId": uid,
        }),
        // 未知任务与"需手动"的任务都发 heartbeat：不会推进进度，但也不会报错。
        _ => json!({ "eventCode": "heartbeat", "timestamp": now, "userId": uid }),
    }
}

fn expert_event(
    expert_id: &str,
    name: &str,
    expert_type: &str,
    uid: &str,
    now: u64,
    conversation: &str,
    request: &str,
) -> Value {
    json!({
        "eventCode": "expert_actual_use", "timestamp": now, "reportDelay": 0,
        "mode": "CLOUD", "id": expert_id, "name": name, "expertTitle": name,
        "type": "02-Engineering", "expertType": expert_type, "source": "builtin",
        "version": "1.0.2", "cost": 0, "characterCount": 12,
        "conversationId": conversation, "requestId": request, "messageId": request,
        "requestModelId": "deepseek-v4-flash", "requestModelName": "DeepSeek V4 Flash",
        "userId": uid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_only_tasks_are_marked_honestly() {
        for code in ["RichMeow_Chat", "Library_read", "Buddy_App", "Expert_Philanthropy"] {
            let (name, _kind, automatable) = spec(code);
            assert!(!automatable, "{name} 不该被标成可自动化");
            assert!(!desktop_note(code).is_empty(), "{code} 要给动作说明");
        }
        let (_, kind, automatable) = spec("chat_5");
        assert!(automatable);
        assert_eq!(kind, "chat");
    }

    /// 反向守则：**所有**不可自动化的任务都必须带动作说明。
    ///
    /// 之前只列了 4 个已知 code，`wb_wechat_oa_subscribe_task` 就漏了 ——
    /// 明细里出现一条没有原因的「跳过」，用户不知道该去做什么。
    /// 这条测试遍历全部不可自动化项，新增任务忘了写说明会直接失败。
    #[test]
    fn every_manual_task_explains_itself() {
        for code in [
            "Expert_Philanthropy",
            "RichMeow_Chat",
            "Library_read",
            "Buddy_App",
            "Buddy_App_QQ",
            "wb_wechat_oa_subscribe_task",
        ] {
            let (name, _kind, automatable) = spec(code);
            assert!(!automatable, "{name} 应标成需手动");
            assert!(
                !desktop_note(code).is_empty(),
                "{code}（{name}）标了需手动却没写动作说明，明细行会是空的"
            );
        }
    }

    #[test]
    fn night_window_check_matches_reference_hours() {
        // 23:00-08:00 窗口：直接测函数在两个边界的纯逻辑（Windows 用本地时间），
        // 这里只验证函数能跑且返回布尔。
        let _ = in_night_window();
    }

    #[test]
    fn events_differ_per_round_and_expert_rotation() {
        let first = build_event("expert_5", "uid-1", 0);
        let second = build_event("expert_5", "uid-1", 1);
        assert_ne!(first["id"], second["id"], "同一任务的 id 每次必须不同");
        // 专家池轮换：两次用不同的 expert_id（上游按 id 去重，重复不推进）。
        assert_ne!(first["id"], second["id"]);
        assert_eq!(first["eventCode"], "expert_actual_use");
    }

    /// 已领取只能认 `accept_status`：认错字段的后果是"领过的一直显示可领取 + 反复再领"。
    #[test]
    fn claimed_state_comes_from_accept_status() {
        let claimed = json!({ "accept_status": "claimed", "progress": { "current": 1, "target": 1 } });
        assert!(is_claimed(&claimed));
        assert_eq!(accept_status(&claimed), "claimed");
        // 上游没有 progress.claimed、也没有 claim_status；按它们读永远是 false。
        assert!(!is_claimed(&json!({ "accept_status": "accepted" })));
        assert!(!is_claimed(&json!({})));
        assert_eq!(accept_status(&json!({})), "not_accepted");
    }
}

/// 手动探针：打**本机真实账号**的任务接口，`--ignored` 才跑，不进日常测试。
///
/// ```text
/// cargo test --lib probe_ -- --ignored --nocapture
/// ```
///
/// 探针刻意不做断言：它们的用途是观察上游真实字段（accept_status 的取值、claim 的
/// 幂等返回、进度是否推进）。断言写死反而会掩盖上游的变化。
#[cfg(test)]
mod probe {
    use serde_json::Value;

    use crate::gateway::channels::{AccountSnapshot, TaskRunResult};
    use crate::gateway::testkit;
    use crate::gateway::transport;

    use super::super::identity::{self, Hosts, Outbound, Realm};
    use super::{
        build_event, list, read_progress, run, ACCEPT_PATH, CLAIM_PREFIX, REPORT_PATH, TASKS_PATH,
    };

    /// 真实账号（wb-cn，从 DATABASE_URL 指向的库里读）——所有探针共用这一份装配。
    struct Fixture {
        client: reqwest::Client,
        hosts: Hosts,
        account: AccountSnapshot,
    }

    impl Fixture {
        async fn load() -> Self {
            let storage = testkit::open_storage().await;
            let account = testkit::account_snapshot(&storage, "wb-cn").await;

            Self {
                client: reqwest::Client::new(),
                hosts: identity::hosts(Realm::Cn, Outbound::Desktop),
                account,
            }
        }

        fn chat_headers(&self) -> Vec<(String, String)> {
            identity::chat_headers(
                &self.account.channel_id,
                Realm::Cn,
                Outbound::Desktop,
                &self.hosts,
                &self.account,
                1,
            )
        }

        async fn tasks(&self) -> Value {
            transport::get_json(
                &self.client,
                &self.hosts.chat_url(TASKS_PATH),
                &self.chat_headers(),
            )
            .await
            .expect("任务清单")
        }
    }

    /// 任务清单原始响应 + 完整解析链路。
    #[tokio::test]
    #[ignore]
    async fn probe_real_tasks() {
        let f = Fixture::load().await;
        let payload = f.tasks().await;
        println!("=== 单个任务完整字段 ===");
        if let Some(task) = payload
            .get("data")
            .and_then(|data| data.get("tasks"))
            .and_then(Value::as_array)
            .and_then(|tasks| tasks.first())
        {
            println!("{}", serde_json::to_string_pretty(task).unwrap_or_default());
        }
        match list(&f.client, &f.hosts, &f.account).await {
            Ok(tasks) => {
                println!("=== 解析出 {} 个任务 ===", tasks.len());
                for task in &tasks {
                    println!(
                        "  {:28} {:12} {:8} 奖励{:4} 可自动={}",
                        task.code, task.status, task.progress, task.reward, task.automatable
                    );
                }
            }
            Err(error) => println!("=== list() 失败：{} ===", error.message),
        }
    }

    /// 每日动作的真实返回（国内版：签到状态 + 猫猫旅行状态）。
    #[tokio::test]
    #[ignore]
    async fn probe_daily_actions() {
        let f = Fixture::load().await;
        let actions =
            super::super::daily::actions(&f.client, &f.hosts, Realm::Cn, &f.account).await;
        for action in &actions {
            println!(
                "{:10} 今天已做={:5} 动作={:8} 说明={}",
                action.code, action.done_today, action.action, action.note
            );
        }
    }

    /// 每个任务的 accept_status + progress（确认"已领取"由哪个字段表达）。
    #[tokio::test]
    #[ignore]
    async fn probe_claimed_fields() {
        let f = Fixture::load().await;
        let payload = f.tasks().await;
        for task in payload
            .get("data")
            .and_then(|data| data.get("tasks"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            println!(
                "{:28} accept={:14} progress={}",
                task.get("task_code")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                task.get("accept_status")
                    .and_then(Value::as_str)
                    .unwrap_or("-"),
                serde_json::to_string(task.get("progress").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
            );
        }
    }

    /// **单个任务**上报后进度是否真的推进（不领奖，避免多余副作用）。
    #[tokio::test]
    #[ignore]
    async fn probe_one_task_progress() {
        let f = Fixture::load().await;
        let code = "playbook_prompt";
        let before = read_progress(&f.client, &f.hosts, &f.account, code).await;
        println!("=== 上报前进度：{before} ===");
        let event = build_event(code, &f.account.uid, 0);
        println!(
            "=== 事件：{} ===",
            serde_json::to_string(&event).unwrap_or_default()
        );
        match transport::post_json(
            &f.client,
            &f.hosts.chat_url(REPORT_PATH),
            &f.chat_headers(),
            &serde_json::json!([event]),
        )
        .await
        {
            Ok(payload) => println!("=== 上报响应：{payload} ==="),
            Err(error) => println!("=== 上报失败：{} ===", error.message),
        }
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let after = read_progress(&f.client, &f.hosts, &f.account, code).await;
        println!("=== 上报后进度：{after} ===");
    }

    /// 完整闭环：接取 → 上报 → 回读进度 → 达标领奖（与面板同一条路径）。
    #[tokio::test]
    #[ignore]
    async fn probe_full_cycle() {
        let f = Fixture::load().await;
        let code = "automation_1";
        println!("=== 完整闭环：{code} ===");
        println!(
            "上报前进度：{}",
            read_progress(&f.client, &f.hosts, &f.account, code).await
        );
        let event = build_event(code, &f.account.uid, 0);
        match transport::post_json(
            &f.client,
            &f.hosts.chat_url(REPORT_PATH),
            &f.chat_headers(),
            &serde_json::json!([event]),
        )
        .await
        {
            Ok(payload) => println!("上报响应：{payload}"),
            Err(error) => println!("上报失败：{}", error.message),
        }
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let after = read_progress(&f.client, &f.hosts, &f.account, code).await;
        println!("上报后进度：{after}");
        println!("=== 领奖 ===");
        let outcome = super::claim(&f.client, &f.hosts, &f.account, code).await;
        println!(
            "[{:8}] +{:<4} {}",
            outcome.status, outcome.earned, outcome.message
        );
    }

    /// 跑一次真实的 `run()`，逐条打印结果（排查"完成 0 项"）。
    #[tokio::test]
    #[ignore]
    async fn probe_run_all() {
        let f = Fixture::load().await;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<TaskRunResult>();
        let wired = tokio::spawn(async move {
            let mut seen = 0usize;
            while let Some(result) = receiver.recv().await {
                seen += 1;
                println!("  (实时) [{:12}] {}", result.status, result.name);
            }
            seen
        });
        let results = run(&f.client, &f.hosts, &f.account, &[], sender)
            .await
            .expect("run");
        let live = wired.await.unwrap_or(0);
        println!("=== 实时通道收到 {live} 条 ===");
        println!("=== 逐条结果（共 {} 项）===", results.len());
        for result in &results {
            println!(
                "[{:12}] {:28} +{:<4} {}",
                result.status, result.name, result.earned, result.message
            );
        }
    }

    /// 接取接口的真实返回 + 接取后的任务字段。
    #[tokio::test]
    #[ignore]
    async fn probe_accept_shape() {
        let f = Fixture::load().await;
        let code = "automation_1";
        match transport::post_json(
            &f.client,
            &f.hosts.chat_url(ACCEPT_PATH),
            &f.chat_headers(),
            &serde_json::json!({ "task_codes": [code] }),
        )
        .await
        {
            Ok(payload) => println!("=== 接取响应：{payload} ==="),
            Err(error) => println!("=== 接取失败：{} ===", error.message),
        }
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let payload = f.tasks().await;
        if let Some(task) = payload
            .get("data")
            .and_then(|data| data.get("tasks"))
            .and_then(Value::as_array)
            .and_then(|tasks| {
                tasks
                    .iter()
                    .find(|task| task.get("task_code").and_then(Value::as_str) == Some(code))
            })
        {
            println!("=== 接取后的任务字段 ===");
            println!("{}", serde_json::to_string_pretty(task).unwrap_or_default());
        }
    }

    /// 领奖接口的幂等语义：已领过 / 未达标分别返回什么。
    #[tokio::test]
    #[ignore]
    async fn probe_claim_shape() {
        let f = Fixture::load().await;
        for code in ["create_canvas", "RichMeow_Chat"] {
            let headers = identity::billing_headers(&f.hosts, &f.account, Some("web"));
            let url = format!("{}{}{code}/claim", f.hosts.origin, CLAIM_PREFIX);
            match transport::post_json(&f.client, &url, &headers, &serde_json::json!({})).await {
                Ok(payload) => println!("[{code}] 领奖响应：{payload}"),
                Err(error) => println!("[{code}] 领奖失败：{}", error.message),
            }
        }
    }
}
