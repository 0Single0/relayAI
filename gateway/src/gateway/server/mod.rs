//! HTTP 服务：监听、路由、端口回退、健康探针。
//!
//! 两个边界面：
//!   · 外部客户端（OpenAI / Anthropic / Responses 协议）走 `/v1/*`；
//!   · Go 管理面走 `/internal/*`（共享密钥鉴权，只绑回环）。
//!
//! **契约是协议，不是客户端**：网关不识别调用方身份，不为某个客户端写分支。

pub mod admin;
pub mod anthropic;
pub mod openai;
pub mod payload;
pub mod responses;
pub mod sse;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::gateway::{
    config::PORT_SCAN_RANGE,
    state::{ServerStatus, SharedGateway},
    timeutil::now_ms,
};

/// 路由表。
///
/// `internal_token` 是 Go 管理面与本进程之间的共享密钥（`/internal/*` 用）。
pub fn router(state: SharedGateway, internal_token: String) -> Router {
    use axum::extract::DefaultBodyLimit;
    // 请求体大小**完全不限制**（axum 默认 2MB 会把长上下文的请求在网关就 413 掉）。
    // 上限交给上游：超了上游会自己报错，我们原样透传 —— 网关不做第二道闸。
    // 注意 layer 的顺序：axum 的 .layer() 包装的是**在此之前**定义的路由，
    // 所以 DefaultBodyLimit 必须放在 .route() 之后调用才能作用到它们身上
    // （放前面 = 作用在空 Router 上 = 完全不生效，这正是 413 一直阴魂不散的原因）。
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(openai::list_models))
        .route("/v1/chat/completions", post(openai::chat_completions))
        // 旧版 completions 与 chat 同管线（协议差异在解码层归一）。
        .route("/v1/completions", post(openai::chat_completions))
        // Anthropic Messages：Claude Code 等客户端；错误信封与事件序列都是 Anthropic 形状。
        .route("/v1/messages", post(anthropic::messages))
        .route("/v1/messages/count_tokens", post(anthropic::count_tokens))
        // OpenAI Responses：Codex 等客户端；无状态网关，previous_response_id 会被拒绝。
        .route("/v1/responses", post(responses::create))
        .merge(admin::router(state.clone(), internal_token))
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

/// 起服务：绑定失败只写状态，不 panic。
pub async fn run(state: SharedGateway, internal_token: String) {
    let config = state.config();
    let (preferred, allow_lan) = (config.port, config.allow_lan);
    let listener = match bind_with_fallback(preferred, allow_lan).await {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!("监听 127.0.0.1:{preferred} 失败：{error}");
            eprintln!("[gateway] {message}");
            state.set_server_status(ServerStatus {
                running: false,
                addr: None,
                error: Some(message),
            });
            return;
        }
    };
    if let Ok(addr) = listener.local_addr() {
        if addr.port() != preferred {
            eprintln!("[gateway] 端口 {preferred} 被占用，已回退到 http://{addr}");
        }
    }
    serve(listener, state, internal_token).await;
}

/// 在已绑定的监听器上提供服务：写服务状态 → 跑 axum。
///
/// 与 `run` 拆开是为了让测试能走同一条生产路径（绑定的端口可以由测试自己挑）。
pub async fn serve(listener: tokio::net::TcpListener, state: SharedGateway, internal_token: String) {
    let addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "127.0.0.1:?".to_string());
    eprintln!("[gateway] 已监听 http://{addr}");
    state.set_server_status(ServerStatus {
        running: true,
        addr: Some(format!("http://{addr}")),
        error: None,
    });

    let app = router(std::sync::Arc::clone(&state), internal_token);
    if let Err(error) = axum::serve(listener, app).await {
        let message = format!("HTTP 服务异常退出：{error}");
        eprintln!("[gateway] {message}");
        state.set_server_status(ServerStatus {
            running: false,
            addr: None,
            error: Some(message),
        });
    }
}

async fn bind_with_fallback(port: u16, allow_lan: bool) -> Result<tokio::net::TcpListener, std::io::Error> {
    // 允许局域网访问时绑 0.0.0.0（IPv4 全网卡）；否则只绑回环。
    // 回环绑定的监听器拿不到 0.0.0.0 地址，所以分两条路径。
    let host = if allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    let mut last_error = None;
    for candidate in port..port.saturating_add(PORT_SCAN_RANGE) {
        match tokio::net::TcpListener::bind((host, candidate)).await {
            Ok(listener) => return Ok(listener),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("没有可用端口")))
}

/// 记录**没能产出响应**的请求（鉴权失败、并发满、协议解码失败、上游全挂）。
///
/// 参考项目靠 `model LIKE '(未进入调度%)'` 在统计里排除这类行，但那个前缀后来
/// 改成了 `(上游失败 …)`，排除条件就失效了 —— 拒绝请求被算进总量、按渠道分组、
/// 按模型分组，面板上多出一个 `channel_id = '-'` 的分组（SPEC 5.5）。
///
/// 现在改用显式布尔列 `is_rejection`：统计只看这一列，语义不依赖任何字符串约定。
pub(super) fn record_rejection(
    state: &SharedGateway,
    path: &str,
    error: &crate::gateway::errors::GatewayError,
) {
    let label = match error.kind {
        crate::gateway::errors::ErrorKind::Upstream5xx
        | crate::gateway::errors::ErrorKind::Network => "上游失败",
        crate::gateway::errors::ErrorKind::NoAccount => "无可用账号",
        crate::gateway::errors::ErrorKind::RateLimited => "限流或并发",
        crate::gateway::errors::ErrorKind::KeyForbidden => "Key 无权限",
        _ => "请求被拒",
    };

    state.storage.record_usage(crate::gateway::storage::UsageRecord {
        request_id: format!("rej-{}", crate::gateway::timeutil::now_ms()),
        key_id: None,
        // model 仍保留一段可读标签，便于日志排查；但统计不再依赖它。
        model: format!("({label} {path})"),
        upstream_model: None,
        channel_id: "-".into(),
        account_uid: "-".into(),
        protocol: "chat".into(),
        status: "rejected".into(),
        is_rejection: true,
        stream: false,
        attempts: 0,
        usage: Default::default(),
        estimated: false,
        ttft_ms: None,
        total_ms: 0,
        effort: None,
        upstream_cost: 0.0,
        billed_cost: 0.0,
        // 拒绝行没进到上游，没有计价过程可言。
        price_snapshot: None,
        error_detail: Some(format!("[{:?}] {}", error.kind, error.describe())),
        // 拒绝行没有请求内容可留（还没进到上游）。
        payload: None,
    });
}

/// 健康探针：只回答"活着吗、能不能干活"。
///
/// 参考项目把渠道/账号/矩阵/决策全塞在这里给面板用；relayAI 的管理面走
/// `/internal/status`，所以这里保持**最小且稳定**的形状：
/// 运维脚本、容器健康检查、负载均衡探活都靠它，字段不宜随业务变化。
async fn health(State(state): State<SharedGateway>) -> Response {
    let status = state.server_status();
    let now = now_ms();

    let ready_accounts: usize = state
        .channels()
        .iter()
        .map(|channel| {
            state
                .pool
                .by_channel(channel.id())
                .iter()
                .filter(|account| account.is_ready("", now))
                .count()
        })
        .sum();

    Json(json!({
        "status": if status.running { "ok" } else { "stopped" },
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeMs": state.uptime_ms(),
        "channels": state.channels().len(),
        "accounts": state.pool.accounts().len(),
        "readyAccounts": ready_accounts,
    }))
    .into_response()
}
