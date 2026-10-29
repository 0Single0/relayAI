//! 渠道适配层：把某个上游订阅服务封装成 [`ChannelAdapter`]。
//!
//! `channels` **不得引用 `core`**：适配器不认识调度器、冷却、亲和，
//! 它只负责「怎么登录 / 怎么刷新 / 怎么发一次请求 / 怎么读回事件 / 这个错误是什么意思」。
//! 新增渠道只改本模块 + 配置。

// 华为云 CodeArts 已整体移除（SPEC 10.3）：上游限制 3 会话并发、
// `tool_choice` 只认字符串，收益不如直接用「自定义供应商」接 ModelArts。
pub mod custom;
pub mod fake;
pub mod openai_compat;
pub mod trae;
pub mod workbuddy;

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;

use crate::gateway::{
    config::ChannelConfig,
    errors::GatewayError,
    protocol::cir::{CanonicalEvent, CanonicalRequest},
    transport::{self, FrameStream, UpstreamRequest},
};

/// 渠道看到的最小账号视图：只有出站需要的字段，拿不到运行态（冷却/在飞）。
#[derive(Debug, Clone, Default)]
pub struct AccountSnapshot {
    pub channel_id: String,
    pub uid: String,
    pub access_token: Option<String>,
    /// 刷新凭据要用；上游每次刷新都会轮换它。
    pub refresh_token: Option<String>,
    pub enterprise_id: Option<String>,
    /// 华为云这类「每请求签名」渠道需要的 AK/SK（随 token 一起轮换）。
    pub access_key_id: Option<String>,
    pub secret_key: Option<String>,
    /// OAuth 刷新时回传的 PKCE verifier。
    pub code_verifier: Option<String>,
    /// 该账号的上游基址（空 = 用适配器内置默认）。
    ///
    /// 为什么在账号上：一份计价策略（渠道）下可以挂多个不同地址的账号
    /// —— 自建中转、官方直连、备用反代。地址是凭据的属性，不是计价的属性。
    pub base_url: Option<String>,
    /// 该账号的上游协议：`openai-chat` / `openai-responses` / `anthropic`。
    ///
    /// 同一把 Key 往往只支持其中一种，所以它跟着凭据走。
    pub protocol: Option<String>,
    /// 该账号的**平台**：`workbuddy` / `trae` / `custom` / `fake`。
    ///
    /// 平台是账号属性，不是渠道属性 —— 一个计价渠道下面可以混不同平台的号。
    /// 适配器注册表按它选实例，不再按渠道选。
    pub platform: Option<String>,
    /// 适配器私有参数（如 workbuddy 的 `realm` / `outbound_identity`）。
    ///
    /// 与 `base_url` / `protocol` 同理：这些描述"这个号是哪个区的、用什么身份出站"，
    /// 属于凭据属性。放在渠道上会导致同渠道内所有号被迫同区。
    pub options: Option<serde_json::Value>,
}

/// 落库的凭据。刷新会轮换 refresh_token，所以整块一起换。
///
/// 需要序列化：它以 JSON 形式装进加密信封再进 SQLite。
///
/// `access_token` 的语义按渠道而定：
/// · Trae / WorkBuddy —— 上游 JWT（`Authorization: Cloud-IDE-JWT …`）；
/// · CodeArts —— **STS security token**（`x-auth-token`）。
///
/// `secret_key` / `access_key_id` 是为华为云这类「AK/SK + HMAC 签名」的渠道准备的：
/// 它们的每个请求都要用临时密钥签名，光有 token 打不动上游。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Credential {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub uid: Option<String>,
    pub nickname: Option<String>,
    pub enterprise_id: Option<String>,
    pub domain: Option<String>,
    /// 华为云临时 Access Key（随 STS token 一起轮换）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    /// 华为云临时 Secret Key。**只在签名时用，绝不外传**。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
    /// OAuth 刷新时要回传的 PKCE verifier（CodeArts 需要它才换得到新 token）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
}

impl Credential {
    /// 这份凭据**能不能拿去发请求**。
    ///
    /// 不能只看 `access_token`：不同渠道的凭据形态不同 ——
    /// · Trae / WorkBuddy：一个 JWT 就够了；
    /// · CodeArts：长期 AK/SK（`credentials.csv`）**没有 token**，靠 AK/SK 签名；
    /// · 自定义供应商：一把静态 API Key（也存在 `access_token` 里）。
    ///
    /// 早先各处都写 `access_token.is_empty()` 判断，结果 CodeArts 一导入就被判成
    /// "没有凭据 / 需要先登录" —— 明明凭据是好的。
    pub fn is_present(&self) -> bool {
        !self.access_token.is_empty()
            || !self.access_key_id.as_deref().unwrap_or_default().is_empty()
    }

    /// 剩余寿命不足 `margin_ms` 就要刷新（选号时的安全边距）。
    pub fn is_expiring(&self, now_ms: u64, margin_ms: u64) -> bool {
        match self.expires_at_ms {
            Some(expires_at) => expires_at.saturating_sub(now_ms) <= margin_ms,
            // 上游没给过期时间时按"需要刷新"处理：宁可多刷一次，不能拿死 token 打上游。
            None => true,
        }
    }
}

/// 渠道模型目录里的一条。能力/长度用于矩阵的候选过滤。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct UpstreamModel {
    pub id: String,
    pub display_name: Option<String>,
    /// 倍率字符串：`x0.00` / `0` 视为免费；缺失表示未知（按收费处理）。
    pub credits: Option<String>,
    pub context_length: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub vendor: Option<String>,
    pub capabilities: ModelCapabilities,
    /// 上下文档位（上游 `contextWindow.supportedLengths`）。
    pub context_windows: Vec<u64>,
    /// 思考档位：可选档位 / 固定档位 / 默认档 / 能否关闭 / 是否强制思考。
    pub reasoning_efforts: Vec<String>,
    pub reasoning_effort_map: HashMap<String, String>,
    pub reasoning_fixed_effort: Option<String>,
    pub reasoning_default_effort: Option<String>,
    pub reasoning_can_disable: Option<bool>,
    pub always_reasoning: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// 这条来自内置快照（而非实时目录）：能力字段与"现在是免费的"都可能过期，
    /// 所以矩阵会把它标成 Builtin 来源，并且不拿它的能力去做硬性过滤。
    pub from_snapshot: bool,
    /// 这条是**用户显式声明**的（自定义供应商的模型清单），不是某个渠道家族的目录。
    ///
    /// 两处必须区别对待，否则自定义供应商会坏掉：
    /// ① 命名清洗（`is_chat_model` 剔掉 `auto`/`lite` 等）是 workbuddy 家族的约定，
    ///    而用户填的模型名就是他要用的名字 —— 一个叫 `auto` 的模型不能静默消失；
    /// ② 能力字段是**参考信息**而不是硬性过滤器：用户只填了模型 ID 时，
    ///    三个能力位都是 false，若据此判定"不支持工具/图片"就会把正常请求拦在本地，
    ///    这与「未知就乐观放行、让上游报错」的既定策略相反。
    pub user_declared: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tool_calls: bool,
    pub reasoning: bool,
    /// 目录里**明确声明过**工具能力吗？
    ///
    /// `tool_calls = false` 有两种截然不同的含义：
    /// · 上游明确说"这个模型不支持工具" → 应当过滤；
    /// · 上游根本没提这个字段 → **不能**当成不支持（乐观放行，让上游自己报错）。
    ///
    /// 只看 bool 无法区分二者，于是出现了"Trae 明明能调工具，却被判成
    /// 不支持工具调用、整个候选被剔掉"的故障（能力来源由 vision/reasoning
    /// 为真推导出来，掩盖了"工具位其实没声明"这件事）。
    #[serde(default)]
    pub declared_tools: bool,
    /// 目录里**明确声明过**图片能力吗？理由同 [`Self::declared_tools`]。
    #[serde(default)]
    pub declared_vision: bool,
}

/// 渠道无关的"运营任务"视图：面板的「任务」表格按它渲染。
///
/// 各渠道口径不同（workbuddy 是成长任务/签到/猫猫旅行，将来的渠道可能完全不同），
/// 所以只归一通用字段；渠道特有的说明放 `note` 原样透出。
/// 自动化做不了的任务也要列出（`automatable=false` + 原因），诚实比假装能做重要。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskInfo {
    /// 渠道内部的任务编号（accept/claim 用）。
    pub code: String,
    pub name: String,
    /// not_accepted / in_progress / claimable / done / unsupported
    pub status: String,
    /// 进度 "2/5"；拿不到就给空串。
    pub progress: String,
    /// 奖励积分（0 = 未标注）。
    pub reward: u32,
    pub automatable: bool,
    /// 不能自动化时的原因 / 需要的动作（如"需在桌面端打开资料库"）。
    pub note: String,
}

/// 一次任务执行的结果行（面板的执行记录）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRunResult {
    pub code: String,
    pub name: String,
    /// done / skipped / failed / not_in_window
    pub status: String,
    pub message: String,
    /// 本轮拿到的积分（未领到为 0）。
    pub earned: u32,
}

/// 一个**每日动作**（签到 / 猫猫旅行 / 每日活跃对话…）。
///
/// 为什么单独一层而不是并进任务清单：任务是"一次性、做完消失"，
/// 每日动作是"每天都要再做一次"。混在一起就会出现"签完显示已完成"——
/// 明天明明还得签。所以面板把它们分开列，契约也分开。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DailyAction {
    /// 渠道内部编号（执行时回传）。
    pub code: String,
    pub name: String,
    /// 用户能做的动作名（"签到" / "领取" / "派出旅行" / "去对话"…）。
    pub action: String,
    /// 今天是否已经做过（true 时按钮置灰）。
    ///
    /// **字段名必须显式 camelCase**：这个结构体默认按 Rust 命名序列化成
    /// `done_today`，而管理面按 `doneToday` 解析 —— 名字对不上，值永远是 false，
    /// 界面就会把"今天已签到"显示成未完成。
    #[serde(rename = "doneToday")]
    pub done_today: bool,
    /// 不能自动完成时的原因 / 补充说明（连续天数、剩余次数…）。
    pub note: String,
    /// 动作是否可自动执行。
    pub automatable: bool,
}

/// 渠道无关的额度快照。
///
/// 各渠道口径天然不同（workbuddy 是积分/资源包，将来 qoder 可能是额度、
/// kiro 可能是试用期或请求数），所以这里只做**归一**，不做解释：
/// 面板按 `label` + `unit` 展示，渠道特有的补充信息放 `details` 原样透出。
/// 新渠道接入时只要实现 `ChannelAdapter::fetch_quota`，面板不用改。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct QuotaSnapshot {
    /// 主口径名称：积分 / 额度 / 余额 / 请求数 …
    pub label: String,
    /// 单位：点 / 元 / 次 / token …
    pub unit: String,
    /// 剩余量。
    pub remaining: Option<f64>,
    /// 周期总量（拿不到就不给）。
    pub total: Option<f64>,
    /// 已用量（拿不到就不给）。
    pub used: Option<f64>,
    /// 渠道特有的补充行，面板按原样展示（键值对）。
    pub details: Vec<(String, String)>,
    /// 抓取时间（epoch ms）：面板据此显示"多久之前"。
    pub fetched_at_ms: u64,
}

/// 登录会话票据：state 由适配器保管在上游，authUrl 交给用户浏览器打开。
#[derive(Debug, Clone)]
pub struct LoginTicket {
    pub state: String,
    pub auth_url: String,
}

/// 登录轮询结果。
#[derive(Debug, Clone)]
pub enum LoginPoll {
    /// 用户还没完成授权（上游 pending 码）。
    Pending,
    /// 授权完成，凭据可以落库。
    Ready(Box<Credential>),
    /// 上游给了明确失败。
    Failed(String),
}

/// 渠道的运营能力（SPEC 11.1）。
///
/// 管理面据此决定「任务」菜单的二级项。**纯函数，不发网络请求**：
/// 每次进页面都跨服务问一遍不划算，而且网关重启期间菜单会空掉。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct OpsCaps {
    /// 有任务清单（可接取/上报/领奖）。
    pub tasks: bool,
    /// 有每日动作清单。
    pub daily: bool,
    /// 有单次签到 hook（没有清单）。
    pub checkin: bool,
}

#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// 渠道实例 id，与 `ChannelConfig::id` 一致。
    fn id(&self) -> &str;
    /// 适配器类型（`workbuddy` / `fake` …），用于面板显示。
    fn adapter_kind(&self) -> &str;

    /// 该渠道有哪些运营能力。默认全 false。
    fn ops_caps(&self) -> OpsCaps {
        OpsCaps::default()
    }

    /// **该账号**有哪些运营能力。默认沿用渠道级能力。
    ///
    /// 为什么要账号级：平台下沉到账号后，一个适配器实例会同时服务多个
    /// 不同配置的账号（WorkBuddy 国内号能跑任务、国际号不能）。
    /// 只看渠道级能力，界面就会给国际号渲染一个点了必然失败的「执行任务」。
    fn ops_caps_for(&self, _account: &AccountSnapshot) -> OpsCaps {
        self.ops_caps()
    }

    /// 该渠道的账号是否必须有凭据才可用（假渠道不需要，用来空手验证链路）。
    fn requires_credential(&self) -> bool {
        true
    }

    /// 凭据是否是**用户填的静态值**（自定义供应商的 API Key），而不是会过期的令牌。
    ///
    /// 为 true 时保鲜链路不再尝试刷新：静态 Key 没有刷新端点，刷新必然失败，
    /// 而"刷新失败"会让调度器把这个账号当作不可用 —— 等于永远用不了。
    fn static_credential(&self) -> bool {
        false
    }

    /// 构建上游报文。纯函数，不碰网络 —— 它是渠道层最值得单测的部分。
    fn build_chat_request(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<UpstreamRequest, GatewayError>;

    /// 一帧上游 SSE → 若干 CIR 事件。未知帧返回空 vec（不因为多一个字段就中断整条流）。
    fn parse_event(&self, frame: &str) -> Vec<CanonicalEvent>;

    /// 把上游状态码/错误码翻译成统一语义。调度器只认 [`GatewayError::kind`] 与 `scope`。
    ///
    /// `upstream_model` 是这次实际发出去的模型名 —— 调度器在构建报文前已经把
    /// [`CanonicalRequest::model`] 改写成了路由的 upstream 名，所以这里拿到的是真名。
    ///
    /// 为什么要传模型名：**限流粒度是渠道自己的事**。workbuddy 的 6004 只限一个模型
    /// （同账号其他模型照用），这个语义只有该渠道知道，所以由它在分类时落到
    /// [`ErrorScope::AccountModel`]；核心调度器不认识任何具体渠道。
    fn classify_error(&self, status: u16, body: &str, upstream_model: &str) -> GatewayError;

    /// 拉模型目录。`Ok(None)` 表示"拉取失败，请回落快照"，**不是**"这个渠道没有模型"。
    async fn list_models(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Vec<UpstreamModel>>, GatewayError>;

    /// 默认传输链路：构建报文 → HTTP → SSE 帧流。就地吐帧的渠道（假渠道）覆盖它。
    async fn chat_stream(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
        client: &reqwest::Client,
    ) -> Result<FrameStream, GatewayError> {
        let upstream = self.build_chat_request(account, request)?;
        match transport::send(&upstream, client).await {
            Ok(response) => Ok(transport::frame_stream(response, STREAM_IDLE_TIMEOUT)),
            Err(transport::UpstreamFailure { status: 0, body }) => {
                Err(GatewayError::network(body))
            }
            // 这里的 request.model 已被调度器改写为路由的 upstream 模型名。
            Err(transport::UpstreamFailure { status, body }) => {
                Err(self.classify_error(status, &body, &request.model))
            }
        }
    }

    /// 刷新凭据。`Ok(None)` = 该渠道不需要刷新（或没有 refresh_token）。
    ///
    /// **调用方必须按账号串行调用**：上游会轮换 refresh_token，并发刷新互相作废。
    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Credential>, GatewayError> {
        Ok(None)
    }

    /// 开始一次登录：拿到 state 与给用户打开的授权链接。
    ///
    /// `realm` 是调用方指定的区域（`cn` / `intl`），**必须与最终建号时的 realm 一致**。
    /// 登录发生在账号还不存在的时候，所以不能靠"读账号的 realm"来决定域名 ——
    /// 过去只能用渠道兜底值（通常是 cn），导致选了国际版也弹国内版的授权页，
    /// 授权完登录进国内站，建出来的号跑不了国际模型。
    async fn login_start(
        &self,
        _client: &reqwest::Client,
        _platform: &str,
        _realm: Option<&str>,
    ) -> Result<LoginTicket, GatewayError> {
        Err(GatewayError::unsupported("该渠道不支持登录流程，请手工填写凭据"))
    }

    /// 轮询登录结果。面板每 2.5 秒调一次。
    async fn login_poll(
        &self,
        _client: &reqwest::Client,
        _state: &str,
    ) -> Result<LoginPoll, GatewayError> {
        Err(GatewayError::unsupported("该渠道不支持登录流程"))
    }

    /// 该渠道的登录是否需要「用户把回调地址粘回来」。
    ///
    /// Trae 这类渠道的授权回调被上游强制为 `http://127.0.0.1:<authorize` 的回环地址，
    /// 而网关没有常驻的本地监听端口 —— 只能由用户复制地址栏内容交回来。
    /// 面板据此把「添加账号」弹窗切到粘贴框，而不是干等一个永远不会来的轮询结果。
    fn login_needs_callback(&self) -> bool {
        false
    }

    /// 该渠道**有没有真正的授权页**。
    ///
    /// 与 `login_needs_callback` 正交：Trae 是「有授权页 + 要粘回调」，
    /// 而 CodeArts 是「压根没有授权页」—— 它要求粘贴一份凭据 JSON / AK-SK。
    /// 没有授权页时，`login_start` 返回的 `auth_url` 只是一行**填写说明**，
    /// 面板绝不能拿它去 `openUrl`（那会打开一个不存在的地址 / 静默失败）。
    fn login_has_auth_page(&self) -> bool {
        true
    }

    /// 用用户粘贴的回调地址完成登录，返回可落库的凭据。
    async fn login_complete(
        &self,
        _client: &reqwest::Client,
        _callback: &str,
    ) -> Result<Credential, GatewayError> {
        Err(GatewayError::unsupported("该渠道不需要粘贴回调地址"))
    }

    /// 运营任务清单（面板「任务」表格）。`Ok(None)` = 该渠道没有任务体系。
    async fn list_tasks(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Vec<TaskInfo>>, GatewayError> {
        Ok(None)
    }

    /// 一键执行可自动化的任务，逐条结果**实时**推到 `progress`，末尾返回全量。
    ///
    /// 走通道而不是只返回 Vec：任务执行是长任务（几十秒到几分钟），
    /// 面板要能一条条看到进展，而不是等全部跑完才弹一个框。
    async fn run_tasks(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
        _only: &[String],
        _progress: tokio::sync::mpsc::UnboundedSender<TaskRunResult>,
    ) -> Result<Vec<TaskRunResult>, GatewayError> {
        Ok(Vec::new())
    }

    /// 单独领取一项**已达标**任务的奖励（面板任务行上的「领取」）。
    ///
    /// 与 `run_tasks` 分开：进度已经达标、只是没领过的任务（比如你在官方端手动做完了），
    /// 不该为了领一次奖把事件再上报一遍。
    async fn claim_task(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
        _code: &str,
    ) -> Result<TaskRunResult, GatewayError> {
        Err(GatewayError::unsupported("该渠道不支持单独领取任务奖励"))
    }

    /// 额度/余额快照。`Ok(None)` = 该渠道没有额度概念，或这次没查到。
    ///
    /// 面板只消费归一后的 [`QuotaSnapshot`]，所以新渠道接额度不需要动界面。
    async fn fetch_quota(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<QuotaSnapshot>, GatewayError> {
        Ok(None)
    }

    /// 该账号今天有哪些每日动作、各自做完了没有。
    ///
    /// `Ok(None)` = 该渠道没有每日动作概念。**只列这个账号真实拥有的动作**：
    /// 国内版有签到/猫猫旅行、国际版有每日活跃对话，两边不重叠 ——
    /// 由渠道自己判断 realm，面板不认识任何具体渠道。
    async fn daily_actions(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<Option<Vec<DailyAction>>, GatewayError> {
        Ok(None)
    }

    /// 执行一个每日动作，返回这一条的结果（成功/失败原因、拿到的积分）。
    async fn run_daily_action(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
        _code: &str,
    ) -> Result<TaskRunResult, GatewayError> {
        Err(GatewayError::unsupported("该渠道不支持每日动作"))
    }

    /// 诊断：用该账号的真实凭据打一次上游，**原样返回响应**。
    ///
    /// 用于"面板显示缺字段"这类问题：只看到 `—`/`?` 时无法判断是上游
    /// 没给、字段名不同，还是接口直接失败。`target` 的含义由渠道自己定义
    /// （Trae 是 models / detail / credits）。
    async fn diagnose(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
        _target: &str,
    ) -> Result<serde_json::Value, GatewayError> {
        Err(GatewayError::unsupported("该渠道没有诊断接口"))
    }

    /// 运营类动作：每日签到（国内版积分）。返回是否算成功（含幂等）。
    ///
    /// 保留这个薄封装：登录流程里会签一次（新号马上拿分）。
    async fn sign_in(
        &self,
        _client: &reqwest::Client,
        _account: &AccountSnapshot,
    ) -> Result<bool, GatewayError> {
        Ok(false)
    }
}

/// 上游目录里哪些 id 算「可对外发布的聊天模型」。
///
/// 这些命名约定在该上游家族里是共通的：`lite`/`auto` 等是内部虚拟模型与 picker 别名，
/// `codewise-*`/`completion-*` 是 IDE 补全通道，带 `-image-alpha*`/`-taco-completion`
/// 后缀的不是通用对话模型。矩阵构建与渠道目录清洗共用同一套规则。
pub fn is_chat_model(id: &str) -> bool {
    const NON_CHAT: [&str; 7] = [
        "lite",
        "default-model",
        "fast-model",
        "balanced-model",
        "primary-model",
        "deep-model",
        "auto",
    ];
    const NON_CHAT_PREFIXES: [&str; 2] = ["codewise-", "completion-"];
    const NON_CHAT_SUFFIXES: [&str; 3] = ["-image-alpha", "-image-alpha-edit", "-taco-completion"];

    !id.is_empty()
        && !NON_CHAT.contains(&id)
        && !NON_CHAT_PREFIXES.iter().any(|prefix| id.starts_with(prefix))
        && !NON_CHAT_SUFFIXES.iter().any(|suffix| id.ends_with(suffix))
}

/// 倍率字符串判定免费：`x0.00` / `x0` / `0` / `0.00`。
/// 缺失或解析不了**不算免费**（继续受额度守卫约束）。
pub fn credits_is_free(credits: Option<&str>) -> bool {
    matches!(
        credits.map(|value| value.trim().to_ascii_lowercase()),
        Some(ref value) if matches!(value.as_str(), "x0.00" | "x0" | "0" | "0.00")
    )
}

/// 上游连上后允许的空闲时间（参考实现：流中 300 秒无数据即视为断流）。
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// 平台注册中心：每个**平台**实例化一个适配器。
///
/// 这是"平台下沉到账号"之后的关键变化：
///   · 过去是 `channel.id -> adapter`，一个渠道一个适配器实例；
///   · 现在是 `platform -> adapter`，所有同平台账号共用一个实例。
///
/// 好处是"加一个 WorkBuddy 账号"不需要先建渠道，也不再需要
/// "WorkBuddy 国内版 / WorkBuddy 国际版"这种把平台配置塞进渠道的假渠道。
/// realm 这类参数改成**按账号**解析（见 `WorkbuddyChannel::identity_for`）。
///
/// 未知平台只记警告并跳过 —— 配置写错不该让整个网关起不来。
/// 内置平台清单。
///
/// **无条件注册**：即使数据库里一条渠道都没有，这些平台也必须可用 ——
/// 否则全新部署时「新建账号」会报「平台不可用」，迈不出第一步。
pub const BUILTIN_PLATFORMS: &[&str] = &["fake", "workbuddy", "custom", "trae"];

pub fn build_registry_from(
    channels: &[ChannelConfig],
    warnings: &mut Vec<String>,
) -> HashMap<String, Arc<dyn ChannelAdapter>> {
    let mut registry: HashMap<String, Arc<dyn ChannelAdapter>> = HashMap::new();

    // 平台集合来自两处：
    //   1. `channels.adapter`（迁移期兼容：老数据只有渠道带 adapter）；
    //   2. **固定必备平台**（内置的几家 + fake）。
    //
    // 第 2 条是必须的，不是"顺手兜底"：
    // 渠道表可能**一条记录都没有**（全新部署，或管理员把分组全删了重建）。
    // 如果平台集合只从渠道表推导，那种状态下适配器注册表就是空的，
    // 于是「新建账号 → 开始授权」会报「平台 workbuddy 不可用」——
    // 而实际上适配器代码一直在，只是没人告诉它该注册。
    //
    // 换句话说：**登录授权不该依赖"已经有账号/渠道"**，
    // 否则全新环境永远迈不出第一步。
    let mut platforms: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();

    for builtin in BUILTIN_PLATFORMS {
        platforms.insert((*builtin).to_string(), serde_json::Value::Null);
    }

    // 渠道里带的 options 作为平台级默认值（真值按账号解析）。
    for channel in channels {
        platforms
            .entry(channel.adapter.clone())
            .or_insert_with(|| channel.options.clone());
    }

    for (platform, options) in platforms {
        // 停用的平台也建适配器：`/v1/models` 与状态页要能看到它，
        // 只是调度时不会选中（候选过滤看账号 enabled）。
        let adapter: Arc<dyn ChannelAdapter> = match platform.as_str() {
            "fake" => Arc::new(fake::FakeChannel::from_options(&platform, &options)),
            "workbuddy" => Arc::new(workbuddy::WorkbuddyChannel::from_options(
                &platform, &options,
            )),
            // 用户自定义的 OpenAI / Anthropic 兼容供应商。
            "custom" => Arc::new(custom::CustomChannel::from_options(&platform, &options)),
            // Trae CN / SOLO CN：两段式会话协议，见 `trae::mod` 的模块说明。
            "trae" => Arc::new(trae::TraeChannel::from_options(&platform, &options)),
            // 华为云 CodeArts 已移除：上游限制 3 会话并发、tool_choice 只认字符串，
            // 收益不如直接用「自定义供应商」接 ModelArts（SPEC 10.3）。
            other => {
                warnings.push(format!("平台 {other} 尚未实现，已跳过"));
                continue;
            }
        };
        registry.insert(platform, adapter);
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **渠道表为空时，内置平台仍然可用。**
    ///
    /// 这是一个真实的线上故障：管理员把分组全删掉之后，
    /// 「新建账号 → 开始授权」报「平台 workbuddy 不可用」——
    /// 因为注册表只从 `channels.adapter` 推导，表空 → 注册表空 → 登录迈不出第一步。
    ///
    /// 换句话说：**登录授权不该依赖"已有账号/渠道"**。
    #[test]
    fn builtin_platforms_exist_even_without_any_channel() {
        let mut warnings = Vec::new();
        let registry = build_registry_from(&[], &mut warnings);

        for platform in BUILTIN_PLATFORMS {
            assert!(
                registry.contains_key(*platform),
                "空渠道表下内置平台 {platform} 必须存在，否则新建账号时无法授权"
            );
        }
    }

    /// 注册表**以平台为键**，不是以渠道为键。
    ///
    /// 这是"平台下沉到账号"的核心：同一个平台只应有一个适配器实例，
    /// 多个渠道共用它。以渠道为键会让"加一个账号"必须先建渠道。
    #[test]
    fn registry_is_keyed_by_platform_not_channel() {
        let channels = vec![
            ChannelConfig {
                id: "qoder".into(),
                adapter: "qoder".into(),
                ..ChannelConfig::default()
            },
            ChannelConfig {
                id: "fake".into(),
                adapter: "fake".into(),
                ..ChannelConfig::default()
            },
            // 两个**不同渠道**、同一个平台：只该产生一个适配器。
            ChannelConfig {
                id: "wb-cn".into(),
                adapter: "workbuddy".into(),
                ..ChannelConfig::default()
            },
            ChannelConfig {
                id: "wb-intl".into(),
                adapter: "workbuddy".into(),
                ..ChannelConfig::default()
            },
        ];

        let mut warnings = Vec::new();
        let registry = build_registry_from(&channels, &mut warnings);

        // 注册表 = 内置平台（无条件）+ 渠道里出现的平台。
        // 这里要验的是"**以平台为键**"和"重复平台合并"，不是总数：
        // 内置平台随时可能增减，把总数写死会让这条测试变成维护负担。
        assert!(
            registry.len() >= BUILTIN_PLATFORMS.len(),
            "内置平台必须全部注册，实际 {}",
            registry.len()
        );
        assert!(registry.contains_key("fake"));
        assert!(registry.contains_key("workbuddy"));
        assert!(
            !registry.contains_key("wb-cn"),
            "注册表不该再以渠道 id 为键"
        );
        assert!(
            !registry.contains_key("wb-intl"),
            "同一平台的第二个渠道也不该另建适配器"
        );
        assert_eq!(warnings.len(), 1, "跳过一个应记一条警告");
        assert!(warnings[0].contains("qoder"));
    }

    #[test]
    fn credential_expiry_margin_is_enforced() {
        let credential = Credential {
            access_token: "t".into(),
            expires_at_ms: Some(1_000_000),
            ..Credential::default()
        };
        // 剩 5 分钟 > 2 分钟边距：还不刷。
        assert!(!credential.is_expiring(1_000_000 - 300_000, 120_000));
        // 剩 60 秒 < 2 分钟边距：要刷。
        assert!(credential.is_expiring(1_000_000 - 60_000, 120_000));
        // 没有过期时间：按需要刷新处理。
        assert!(Credential::default().is_expiring(0, 120_000));
    }

    #[test]
    fn ops_caps_are_declared_per_adapter() {
        // fake 没有任何运营能力（SPEC 11.1）。
        let channels = vec![ChannelConfig {
            id: "fake".into(),
            adapter: "fake".into(),
            ..ChannelConfig::default()
        }];
        let mut warnings = Vec::new();
        let registry = build_registry_from(&channels, &mut warnings);
        let adapter = registry.get("fake").expect("fake 适配器");

        let caps = adapter.ops_caps();
        assert!(!caps.tasks);
        assert!(!caps.daily);
        assert!(!caps.checkin);
    }
}
