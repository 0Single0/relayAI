//! 内部接口：供 Go 管理面调用。
//!
//! 这些操作**必须贴着适配器**才能做（登录、刷新、探活、额度、任务），
//! 所以不能放在 Go 侧。其余 CRUD 由 Go 直连 PG，不经过这里。
//!
//! 鉴权：共享密钥 `X-Internal-Token`。只绑回环，但服务端也可能配 `0.0.0.0`，
//! 所以密钥是必须的，不是可选的。

use std::sync::Arc;

mod account_models;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::gateway::{
    channels::LoginPoll,
    core::{dispatch, refresh},
    errors::GatewayError,
    state::SharedGateway,
    timeutil::now_ms,
};

/// 共享密钥的请求头名。
const TOKEN_HEADER: &str = "x-internal-token";

/// 构建 `/internal/*` 路由。
pub fn router(state: SharedGateway, token: String) -> Router<SharedGateway> {
    let token = Arc::new(token);

    Router::new()
        .route("/internal/status", get(status))
        .route("/internal/catalog", get(catalog))
        .route("/internal/prices", post(set_prices))
        .route("/internal/secrets/seal", post(seal_secret))
        .route("/internal/decisions", get(decisions))
        .route("/internal/matrix/refresh", post(refresh_matrix))
        .route("/internal/channels/{id}/models", get(channel_models))
        .route("/internal/model-discoveries", post(discover_models))
        .route("/internal/channels/{id}/caps", get(channel_caps))
        .route("/internal/channels/{id}/probe", post(probe_channel))
        .route("/internal/accounts", post(create_account))
        .route("/internal/accounts/{key}/refresh", post(refresh_account))
        .route("/internal/accounts/{key}/models", get(account_models::get))
        .route("/internal/accounts/{key}/probe", post(probe_account))
        // 同一路径挂 GET + POST：GET 读内存快照，POST 强制打上游刷新。
        .route(
            "/internal/accounts/{key}/quota",
            get(account_quota).post(refresh_account_quota),
        )
        .route("/internal/accounts/{key}/reset", post(reset_account_cooldowns))
        .route("/internal/channels/{id}/login/start", post(login_start))
        .route("/internal/channels/{id}/login/poll", post(login_poll))
        .route("/internal/channels/{id}/login/complete", post(login_complete))
        .route("/internal/tasks/channels/{id}/live", get(task_live))
        .route("/internal/tasks/run", post(task_run))
        .route("/internal/tasks/cancel", post(task_cancel))
        .route("/internal/tasks/progress", get(task_progress))
        .route("/internal/tasks/claim", post(task_claim))
        .route("/internal/state/reload", post(state_reload))
        .layer(axum::middleware::from_fn(
            move |headers: HeaderMap, request: axum::extract::Request, next: axum::middleware::Next| {
                let expected = Arc::clone(&token);
                async move {
                    let provided = headers
                        .get(TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("");

                    if !crate::gateway::errors::secret_eq(&expected, provided) {
                        return (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({ "error": "内部接口密钥不匹配" })),
                        )
                            .into_response();
                    }

                    next.run(request).await
                }
            },
        ))
        .with_state(state)
}

fn internal_error(error: GatewayError) -> Response {
    (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({
            "error": error.message,
            "kind": format!("{:?}", error.kind),
        })),
    )
        .into_response()
}

/// 网关运行状态。
async fn status(State(state): State<SharedGateway>) -> Response {
    let config = state.config();
    let now = now_ms();

    let channels: Vec<Value> = state
        .channels()
        .iter()
        .map(|channel| {
            let accounts = state.pool.by_channel(channel.id());
            let ready = accounts
                .iter()
                .filter(|account| account.is_ready("", now))
                .count();
            json!({
                "id": channel.id(),
                "adapter": channel.adapter_kind(),
                "accounts": accounts.len(),
                "ready": ready,
            })
        })
        .collect();

    // 账号级运行态。
    //
    // 为什么必须在这里给：管理面的账号列表要显示"在飞 N/M"，
    // 而 `max_inflight` 在库里可以是 NULL（表示"用渠道默认值"）。
    // NULL 不等于"无限"——网关实际用的是 `concurrency.per_account`。
    // 只把库里的 NULL 透给前端，界面就会显示成 `0/∞`，
    // 那是在说"这个账号不限并发"，与真实行为相反。
    // 所以这里把**生效值**算出来一起返回。
    let accounts: Vec<Value> = state
        .pool
        .accounts()
        .iter()
        .map(|account| {
            let blocked = account.blocked_reasons("", now);
            // 额度快照：网关在 ops 巡检里每 5 分钟刷新一次，这里只是读内存，
            // 不打上游。所以把它一起带上不增加任何请求成本，
            // 而账号列表能直接显示"还剩多少积分"，不用再点进详情页。
            let quota = account.quota();
            json!({
                "channelId": account.key.channel_id,
                "uid": account.key.uid,
                "platform": account.platform,
                "enabled": account.enabled,
                "inflight": account.inflight(),
                "maxInflight": account.max_inflight(),
                "ready": account.is_ready("", now),
                "blockedReasons": blocked,
                "cooldownUntilMs": account.cooldown_until_ms(),
                /*
                  模型级限流明细。

                  上游按模型限额是常态。只给账号级的 `cooldownUntilMs`
                  会让"某个模型在限流"看起来像"整个号废了"，
                  用户据此删号重建反而丢了配置。

                  每项形如 `{"model":"glm-5.3","untilMs":1793168000000}`，
                  只含**尚未恢复**的。
                */
                "modelLimits": account
                    .model_cooldowns_snapshot(now)
                    .into_iter()
                    .map(|(model, until)| json!({ "model": model, "untilMs": until }))
                    .collect::<Vec<_>>(),
                "requiresCredential": account.requires_credential,
                "hasCredential": account.credentials().is_present(),
                // 静态凭据（自定义供应商的 API Key）没有"刷新"这回事：
                // 前端据此决定刷新按钮是启用还是禁用，而不是点了才报错。
                "staticCredential": account.static_credential,
                "quota": quota.map(|q| json!({
                    "label": q.label,
                    "unit": q.unit,
                    "remaining": q.remaining,
                    "total": q.total,
                    "used": q.used,
                    "fetchedAtMs": q.fetched_at_ms,
                })),
            })
        })
        .collect();

    let server = state.server_status();
    let matrix = state.matrix_snapshot();
    let (live_routes, builtin_routes, route_count, matrix_age_ms) = match &matrix {
        Some(matrix) => {
            let (live, builtin) = matrix.sources();
            (
                live,
                builtin,
                matrix.route_count(),
                now.saturating_sub(matrix.built_at_ms()),
            )
        }
        None => (0, 0, 0, 0),
    };

    Json(json!({
        "running": server.running,
        "addr": server.addr,
        "error": server.error,
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeMs": state.uptime_ms(),
        "auth": if config.api_keys.is_empty() { "api_key" } else { "static" },
        "concurrency": {
            "global": config.concurrency.global,
            "available": state.global_available(),
        },
        "matrix": {
            "routes": route_count,
            "live": live_routes,
            "builtin": builtin_routes,
            "ageMs": matrix_age_ms,
            "ttlSeconds": config.catalog_ttl_seconds,
        },
        "stats": {
            "requests": state.stats.requests.load(std::sync::atomic::Ordering::Relaxed),
            "failures": state.stats.failures.load(std::sync::atomic::Ordering::Relaxed),
            "retries": state.stats.retries.load(std::sync::atomic::Ordering::Relaxed),
        },
        "channels": channels,
        "accounts": accounts,
        "warnings": state.warnings(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SealBody {
    plaintext: String,
}

#[derive(Debug, Deserialize)]
struct PriceEntry {
    /// 模型名（网关按此名做精确/前缀匹配）。
    model: String,
    /// 元 / 百万 token。
    input: f64,
    output: f64,
    #[serde(default)]
    cache_read: f64,
}

#[derive(Debug, Deserialize)]
struct PricesBody {
    prices: Vec<PriceEntry>,
}

/// 接收管理面推送的模型价格表。
///
/// 网关不查 `models` 表（存储层只读自己需要的列），所以价格由管理面推过来。
/// 计价时用**上游模型名**匹配，因此这里存的是上游名。
///
/// 幂等：每次全量替换。管理面改完价格会调 `notifyReload`，
/// 走的就是这条路径 —— 不推的话网关还在用旧价，账会一直错。
async fn set_prices(
    State(state): State<SharedGateway>,
    Json(body): Json<PricesBody>,
) -> Response {
    let mut map = std::collections::HashMap::with_capacity(body.prices.len());
    for entry in body.prices {
        map.insert(
            entry.model,
            crate::gateway::core::billing::ModelPrice {
                input: entry.input.max(0.0),
                output: entry.output.max(0.0),
                cache_read: entry.cache_read.max(0.0),
            },
        );
    }

    let count = map.len();
    state.set_price_book(crate::gateway::core::billing::PriceBook::new(map));

    Json(json!({ "count": count })).into_response()
}

/// 模型目录：把内存里的「模型 × 渠道」矩阵导出给管理面。
///
/// 为什么需要它：模型清单**只存在于网关的矩阵里**（从渠道适配器实时拉取），
/// 从来没有落进 `models` 表。管理面的模型页与公开的模型广场都读那张表，
/// 于是永远是空的 —— 这就是"数据是假的"的根因。
///
/// 管理面拿到这份目录后按 `public_model` 聚合写入 `models`，
/// 价格与倍率仍由管理面算（网关不管计价）。
async fn catalog(State(state): State<SharedGateway>) -> Response {
    let matrix = state.ensure_matrix(false).await;

    let mut models: Vec<Value> = Vec::new();
    for public_model in matrix.public_models() {
        let routes = matrix.all_routes(&public_model);
        let Some(first) = routes.first() else {
            continue;
        };

        // 同一对外模型可能有多条路由（多渠道路由）。取第一条的能力与档位 ——
        // 同一模型的能力在各渠道间通常一致，差异在价格与优先级。
        let channels: Vec<Value> = routes
            .iter()
            .map(|route| {
                json!({
                    "channelId": route.channel_id,
                    "upstreamModel": route.upstream_model,
                    "priority": route.priority,
                    "weight": route.weight,
                    "enabled": route.enabled,
                    "free": route.is_free(),
                    "source": match route.source {
                        crate::gateway::core::matrix::RouteSource::Live => "live",
                        crate::gateway::core::matrix::RouteSource::Builtin => "builtin",
                        crate::gateway::core::matrix::RouteSource::Override => "override",
                    },
                })
            })
            .collect();

        models.push(json!({
            "name": public_model,
            "displayName": first.display_name.as_deref().unwrap_or(&public_model),
            "contextWindows": if first.context_windows.is_empty() { first.limits.context.into_iter().collect::<Vec<_>>() } else { first.context_windows.clone() },
            "reasoningEfforts": first.reasoning_efforts,
            "capabilities": {
                "vision": first.capabilities.vision,
                "tools": first.capabilities.tool_calls,
                "reasoning": first.capabilities.reasoning,
                // 能力字段的来源：`live` / `snapshot` / `none`。
                //
                // 界面据此把"我们没数据"（none）与"我们按内置对照表填的"（snapshot）
                // 分开显示 —— 早先两者都被压成 `known: false`，卡片上于是出现
                // 「工具调用 视觉 推理」和「能力未知」并列的自相矛盾。
                "source": first.caps_source,
            },
            "maxOutput": first.limits.max_output,
            "channels": channels,
        }));
    }

    Json(json!({ "models": models })).into_response()
}

/// 用主密钥加密一段文本。
///
/// 存在的理由：主密钥只在网关进程里（管理面被攻破时不该连上游凭据一起暴露）。
/// 所以代理密码这类需要落库的机密，必须由 Go 送过来、网关加密后回传。
///
/// **只走内部接口**（需要 `x-internal-token`），不对外。
async fn seal_secret(
    State(state): State<SharedGateway>,
    Json(body): Json<SealBody>,
) -> Response {
    match crate::gateway::security::envelope::seal(&state.storage.master_key(), &body.plaintext) {
        Ok(cipher) => Json(json!({ "cipher": cipher })).into_response(),
        Err(error) => crate::gateway::errors::GatewayError::internal(format!("加密失败：{error}"))
            .into_response(),
    }
}

/// 最近的调度决策。
async fn decisions(
    State(state): State<SharedGateway>,
    Query(query): Query<LimitQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50).min(200);
    let mut list = state.decisions();
    list.truncate(limit);
    Json(list).into_response()
}

/// 强制重建模型矩阵。
async fn refresh_matrix(State(state): State<SharedGateway>) -> Response {
    let matrix = state.ensure_matrix(true).await;
    let (live, builtin) = matrix.sources();
    Json(json!({
        "routes": matrix.route_count(),
        "live": live,
        "builtin": builtin,
    }))
    .into_response()
}

/// 某渠道的模型目录（实时拉取）。
async fn channel_models(
    State(state): State<SharedGateway>,
    Path(id): Path<String>,
) -> Response {
    let mut models = Vec::new();
    for account in state.pool.by_channel(&id).iter().filter(|account| account.enabled) {
        match state.models_for_account(account).await {
            Ok(entries) => {
                for model in entries {
                    if !models.iter().any(|existing: &crate::gateway::channels::UpstreamModel| existing.id == model.id) {
                        models.push(model);
                    }
                }
            }
            Err(error) => return internal_error(error),
        }
    }
    Json(json!(models)).into_response()
}

#[derive(Deserialize)]
struct DiscoverModelsBody {
    base_url: String,
    api_key: String,
    protocol: String,
    account_id: Option<i64>,
}

async fn discover_models(State(state): State<SharedGateway>, Json(body): Json<DiscoverModelsBody>) -> Response {
    let mut api_key = body.api_key;
    if api_key.trim().is_empty() {
        let Some(id) = body.account_id else {
            return (StatusCode::BAD_REQUEST, "请填写 API Key").into_response();
        };
        let cipher = sqlx::query_scalar::<_, Option<Vec<u8>>>(
            "SELECT credential_cipher FROM accounts WHERE id = $1 AND platform = 'custom'",
        ).bind(id).fetch_optional(state.storage.pool()).await;
        let cipher = match cipher {
            Ok(Some(Some(value))) => value,
            Ok(_) => return (StatusCode::BAD_REQUEST, "账号没有可用的 API Key").into_response(),
            Err(error) => return internal_error(GatewayError::internal(format!("读取账号凭据失败：{error}"))),
        };
        let credential = String::from_utf8(cipher).ok().and_then(|sealed| {
            crate::gateway::security::envelope::open(&state.storage.master_key(), &sealed).ok()
        }).and_then(|plain| serde_json::from_str::<crate::gateway::channels::Credential>(&plain).ok());
        let Some(credential) = credential else {
            return internal_error(GatewayError::internal("账号凭据无法解密，请重新设置 API Key"));
        };
        api_key = credential.access_token;
    }
    match crate::gateway::channels::custom::fetch_models(
        &state.client, &body.base_url, None, &api_key, &body.protocol,
    ).await {
        Ok(models) => Json(json!(models)).into_response(),
        Err(error) => error.into_response(),
    }
}

/// 渠道的运营能力（纯函数，不打上游）。
///
/// 管理面据此决定「任务」菜单的二级项；缓存到 PG，网关不可用时菜单仍稳定。
async fn channel_caps(State(state): State<SharedGateway>, Path(id): Path<String>) -> Response {
    let Some(adapter) = state.adapter_for_channel_async(&id).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "渠道不存在" }))).into_response();
    };

    let caps = adapter.ops_caps();
    Json(json!({
        "tasks": caps.tasks,
        "daily": caps.daily,
        "checkin": caps.checkin,
    }))
    .into_response()
}

/// 渠道级探活：用该渠道第一个账号发一条最小请求。
async fn probe_channel(State(state): State<SharedGateway>, Path(id): Path<String>) -> Response {
    let Some(account) = state.pool.by_channel(&id).first().cloned() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "该渠道还没有账号" })),
        )
            .into_response();
    };

    let started = std::time::Instant::now();
    match dispatch::probe_account(&state, &account, None, std::time::Duration::from_secs(45)).await
    {
        Ok(outcome) => Json(json!({
            "ok": true,
            "model": outcome.model,
            "latencyMs": outcome.latency_ms,
            "reply": outcome.reply,
            "error": null,
        }))
        .into_response(),
        Err(error) => Json(json!({
            "ok": false,
            "model": "",
            "latencyMs": started.elapsed().as_millis() as u64,
            "reply": "",
            "error": error.describe(),
        }))
        .into_response(),
    }
}

/// 账号 key 形如 `wb-cn/u_abc`（URL 编码后可能是 `%2F`）。
fn split_key(key: &str) -> (String, String) {
    match key.split_once('/') {
        Some((channel, uid)) => (channel.to_string(), uid.to_string()),
        None => (key.to_string(), String::new()),
    }
}

/// 强制刷新凭据（面板"刷新凭据"按钮）。
async fn refresh_account(
    State(state): State<SharedGateway>,
    Path(key): Path<String>,
) -> Response {
    let (channel_id, uid) = split_key(&key);
    let Some(account) = state.account(&channel_id, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    match refresh::refresh(&state, &account, true).await {
        Ok(credential) => Json(json!({
            "ok": true,
            "expiresAt": credential.expires_at_ms,
            "message": "凭据已刷新",
        }))
        .into_response(),
        Err(error) => internal_error(error),
    }
}

/// 账号探活：真实打上游。
/// 查询参数：探活用哪个模型。
///
/// 不传则按「免费优先」自动挑（探针会真实消耗积分，不该默认拿付费档试）。
/// 传了就用指定的 —— 界面允许改，是因为有些分组的免费模型并不代表真实能力。
#[derive(Debug, Deserialize)]
struct ProbeQuery {
    model: Option<String>,
}

async fn probe_account(
    State(state): State<SharedGateway>,
    Path(key): Path<String>,
    Query(query): Query<ProbeQuery>,
) -> Response {
    let (channel_id, uid) = split_key(&key);
    let Some(account) = state.account(&channel_id, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    let started = std::time::Instant::now();
    let requested = query
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match dispatch::probe_account(&state, &account, requested, std::time::Duration::from_secs(45)).await
    {
        Ok(outcome) => Json(json!({
            "ok": true,
            // 回报实际用的模型：自动模式下由网关挑，用户有权知道"这个绿灯是哪条模型测的"。
            "model": outcome.model,
            "latencyMs": outcome.latency_ms,
            "reply": outcome.reply,
            "error": null,
        }))
        .into_response(),
        Err(error) => Json(json!({
            "ok": false,
            "model": "",
            "latencyMs": started.elapsed().as_millis() as u64,
            "reply": "",
            "error": error.describe(),
        }))
        .into_response(),
    }
}

/// 额度快照。
async fn account_quota(State(state): State<SharedGateway>, Path(key): Path<String>) -> Response {
    let (channel_id, uid) = split_key(&key);
    let Some(account) = state.account(&channel_id, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    match account.quota() {
        Some(quota) => Json(json!(quota)).into_response(),
        None => Json(Value::Null).into_response(),
    }
}

/// 强制刷新账号额度（面板「刷新积分」按钮 + 30s 自动轮询）。
///
/// 与 [`account_quota`]（读内存快照）不同，这里真实打一次上游的额度接口。
/// 静态凭据与自定义供应商没有额度概念（`fetch_quota` 返回 `Ok(None)`），
/// 返回 `null` 让前端如实显示"—"，而不是报错。
async fn refresh_account_quota(
    State(state): State<SharedGateway>,
    Path(key): Path<String>,
) -> Response {
    let (channel_id, uid) = split_key(&key);
    let Some(account) = state.account(&channel_id, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    let Some(adapter) = state.adapter_for(&account) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号没有可用的平台适配器" }))).into_response();
    };

    match adapter.fetch_quota(&state.client, &account.snapshot()).await {
        Ok(snapshot) => {
            // 快照带时间戳落进内存：GET /quota 与状态接口下次读的就是这份。
            account.set_quota(snapshot);
            Json(json!({ "ok": true })).into_response()
        }
        Err(error) => internal_error(error),
    }
}

/// 手动清掉该账号的全部冷却（账号级 + 模型级 + 退避计数）。
///
/// 用途：上游恢复得比我们的退避策略早时，自动等待会让号白闲着。
/// 管理员确认"其实已经能用"就该能立刻放行，而不是干等到点。
///
/// 返回 `cleared` 表示**是否真的清掉了东西**：
/// 没有冷却时点它是无害的，但界面该知道"本来就没在冷却"。
async fn reset_account_cooldowns(
    State(state): State<SharedGateway>,
    Path(key): Path<String>,
) -> Response {
    let (channel_id, uid) = split_key(&key);
    let Some(account) = state.account(&channel_id, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    let cleared = account.reset_cooldowns();
    Json(json!({ "cleared": cleared })).into_response()
}

#[derive(Debug, Deserialize)]
struct LoginStartBody {
    /// 上游平台（`workbuddy` / `trae`）。决定用哪个适配器登录。
    platform: Option<String>,
    /// 登录用的客户端平台标识（`windows` 等），与上面的 `platform` 不是一回事。
    login_platform: Option<String>,
    /// 区域（`cn` / `intl`）。
    ///
    /// 决定授权页走哪个域名：国内 `copilot.tencent.com`，国际 `workbuddy.ai`。
    /// 建号时必须用同一个值，否则凭据与域名对不上。
    realm: Option<String>,
}

/// 发起登录：返回 state 与授权链接。
///
/// 平台由调用方指定（`body.platform`）。不再从渠道推断 ——
/// 平台已经下沉到账号，一个渠道下可能混多平台；登录发生在建号之前，
/// 调用方明确知道"我要加哪家的号"。
async fn login_start(
    State(state): State<SharedGateway>,
    Path(id): Path<String>,
    body: Option<Json<LoginStartBody>>,
) -> Response {
    let parsed = body.map(|Json(value)| value);
    let platform = parsed
        .as_ref()
        .and_then(|value| value.platform.clone())
        .unwrap_or_default();

    if platform.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "必须指定平台" })),
        )
            .into_response();
    }

    let Some(adapter) = state.channel(platform.trim()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("平台 {platform} 不可用") })),
        )
            .into_response();
    };

    let login_platform = parsed
        .as_ref()
        .and_then(|value| value.login_platform.clone())
        .unwrap_or_else(|| "windows".to_string());

    /*
      把 realm 透传给适配器 —— 授权页的域名由它决定。

      不透传就等于"用户选什么都走国内站"，那是这个 bug 的直接原因。
    */
    let realm = parsed
        .as_ref()
        .and_then(|value| value.realm.clone())
        .filter(|value| !value.trim().is_empty());

    match adapter
        .login_start(&state.client, &login_platform, realm.as_deref())
        .await
    {
        Ok(ticket) => Json(json!({
            "state": ticket.state,
            "authUrl": ticket.auth_url,
            "needsCallback": adapter.login_needs_callback(),
            "hasAuthPage": adapter.login_has_auth_page(),
            "hint": if adapter.login_has_auth_page() {
                "打开授权页完成登录"
            } else {
                "该平台没有授权页，请手工填写凭据"
            },
        }))
        .into_response(),
        Err(error) => internal_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct LoginPollBody {
    state: String,
    /// 平台。登录流程无状态（网关不为每个 ticket 存平台），
    /// 所以每次都从请求里带 —— 少了它就拿不到适配器。
    platform: Option<String>,
    options: Option<Value>,
}

/// 轮询登录结果。
async fn login_poll(
    State(state): State<SharedGateway>,
    Path(id): Path<String>,
    Json(body): Json<LoginPollBody>,
) -> Response {
    let platform = body.platform.clone().unwrap_or_default();
    let Some(adapter) = state.channel(platform.trim()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "平台不可用或未指定" })),
        )
            .into_response();
    };

    match adapter.login_poll(&state.client, &body.state).await {
        Ok(LoginPoll::Pending) => Json(json!({
            "state": "pending",
            "message": "等待授权完成",
        }))
        .into_response(),
        Ok(LoginPoll::Ready(credential)) => {
            match persist_login(&state, &id, adapter.adapter_kind(), *credential, body.options.clone()).await {
                Ok(uid) => Json(json!({
                    "state": "ready",
                    "message": "授权完成，账号已添加",
                    "uid": uid,
                }))
                .into_response(),
                Err(error) => internal_error(error),
            }
        }
        Ok(LoginPoll::Failed(message)) => Json(json!({
            "state": "failed",
            "message": message,
        }))
        .into_response(),
        Err(error) => internal_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct LoginCompleteBody {
    state: String,
    callback: String,
    platform: Option<String>,
    options: Option<Value>,
}

/// 用用户粘贴的回调地址完成登录（Trae 这类回环回调）。
async fn login_complete(
    State(state): State<SharedGateway>,
    Path(id): Path<String>,
    Json(body): Json<LoginCompleteBody>,
) -> Response {
    let platform = body.platform.clone().unwrap_or_default();
    let Some(adapter) = state.channel(platform.trim()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "平台不可用或未指定" })),
        )
            .into_response();
    };

    let _ = body.state;
    match adapter.login_complete(&state.client, &body.callback).await {
        Ok(credential) => match persist_login(&state, &id, adapter.adapter_kind(), credential, body.options).await {
            Ok(uid) => Json(json!({
                "state": "ready",
                "message": "授权完成，账号已添加",
                "uid": uid,
            }))
            .into_response(),
            Err(error) => internal_error(error),
        },
        Err(error) => internal_error(error),
    }
}

/// 用静态凭据建号。
///
/// 凭据在这一层加密（主密钥只在网关进程里，SPEC I1）。
///
/// `platform` / `base_url` / `protocol` / `options` 写到**账号**行上而不是渠道：
/// 一份计价策略（渠道）下面可以挂多个不同上游、不同平台的 Key ——
/// 自建中转、官方直连、备用反代。这些是凭据的属性，不是计价的属性。
async fn create_account(
    State(state): State<SharedGateway>,
    Json(body): Json<CreateAccountBody>,
) -> Response {
    /*
      `channel_id` 允许为空：号可以先建、之后再分配分组。

      为什么放宽：建号时（尤其批量导入、授权刚完成）常常还没想好归到哪个计价桶。
      强制先选分组会催生"为了建号随便建一个分组"的脏数据。

      空分组的账号在 `list_accounts` 里照常加载，
      但不会命中任何真实 channel —— 天然不参与调度，凭据与用量归属都保留。
      界面上单独标识为"待分配"，之后在账号列表里批量分配。
    */
    if body.uid.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "账号标识不能为空" })),
        )
            .into_response();
    }
    if body.platform.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "必须指定平台" })),
        )
            .into_response();
    }

    let base_url = body.base_url.unwrap_or_default().trim().to_string();
    let protocol = body
        .protocol
        .unwrap_or_else(|| "openai-chat".into())
        .trim()
        .to_string();
    let options = body.options.unwrap_or_else(|| json!({}));

    let credential = crate::gateway::channels::Credential {
        access_token: body.api_key.clone(),
        ..Default::default()
    };

    // 先落行（带上平台/地址/协议/参数），再写密文。
    // 顺序反过来的话，写密文时行还不存在，UPDATE 会影响 0 行且不报错 ——
    // 账号建出来了但没有凭据，看起来像"登录失败"。
    let inserted = sqlx::query(
        "INSERT INTO accounts
             (channel_id, uid, nickname, enabled, source, base_url, protocol, platform, options, model_allow)
         VALUES (NULLIF($1, ''), $2, $3, true, 'manual', $4, $5, $6, $7, $8)
         ON CONFLICT (uid) DO UPDATE SET
             nickname = EXCLUDED.nickname,
             base_url = EXCLUDED.base_url,
             protocol = EXCLUDED.protocol,
             platform = EXCLUDED.platform,
             options  = EXCLUDED.options,
             model_allow = EXCLUDED.model_allow,
             channel_id = COALESCE(EXCLUDED.channel_id, accounts.channel_id)",
    )
    .bind(body.channel_id.trim())
    .bind(&body.uid)
    .bind(&body.nickname)
    .bind(&base_url)
    .bind(&protocol)
    .bind(body.platform.trim())
    .bind(&options)
    .bind(&body.model_allow)
    .execute(state.storage.pool())
    .await;

    if let Err(error) = inserted {
        return internal_error(GatewayError::internal(format!("账号落库失败：{error}")));
    }

    // 有 Key 才写凭据。OAuth 类型的账号在这里没有 Key，
    // 凭据由登录流程回写；此时把 `credential_cipher` 覆盖成空会让
    // 已登录的号突然变"无凭据"。
    if !body.api_key.trim().is_empty() {
        // 按 uid 定位：未分配分组的行 channel_id 是 NULL，
        // (channel_id, uid) 的 WHERE 匹配不到它。
        if let Err(error) = state
            .storage
            .set_credential_by(&body.channel_id, &body.uid, Some(&credential))
            .await
        {
            return internal_error(error);
        }
    }

    match state.reload_accounts().await {
        Ok(_) => Json(json!({ "ok": true, "uid": body.uid })).into_response(),
        Err(error) => internal_error(error),
    }
}

/// `POST /internal/accounts` 的请求体。
#[derive(Debug, Deserialize)]
struct CreateAccountBody {
    channel_id: String,
    uid: String,
    #[serde(default)]
    nickname: String,
    /// 上游密钥。可以为空：`workbuddy` / `trae` 走 OAuth 登录，
    /// 由 `persist_login` 写凭据，不走这个接口。
    #[serde(default)]
    api_key: String,
    /// 平台：`workbuddy` / `trae` / `custom` / `fake`。
    #[serde(default)]
    platform: String,
    base_url: Option<String>,
    protocol: Option<String>,
    /// 适配器私有参数（workbuddy 的 realm / outbound_identity）。
    options: Option<serde_json::Value>,
    #[serde(default)]
    model_allow: Vec<String>,
}

/// 把登录拿到的凭据落库并重建号池。
///
/// `platform` 必须一起写：账号没有平台就选不到适配器，登录成功也转发不了。
/// 它来自发起登录时那个渠道对应的平台（`/internal/channels/{id}/login/*`）。
async fn persist_login(
    state: &SharedGateway,
    channel_id: &str,
    platform: &str,
    credential: crate::gateway::channels::Credential,
    options: Option<Value>,
) -> Result<String, GatewayError> {
    let channel_id = if channel_id == "__unassigned__" { "" } else { channel_id };
    let options = options.unwrap_or_else(|| json!({}));
    let uid = credential
        .uid
        .clone()
        .ok_or_else(|| GatewayError::internal("上游没有返回账号 uid，无法建号"))?;

    let nickname = credential.nickname.clone().unwrap_or_else(|| uid.clone());

    sqlx::query(
        "INSERT INTO accounts (channel_id, uid, nickname, enabled, source, platform, options)
         VALUES (NULLIF($1, ''), $2, $3, true, 'login', $4, $5)
         ON CONFLICT (uid) DO UPDATE SET
             nickname = EXCLUDED.nickname,
             platform = EXCLUDED.platform,
             options = EXCLUDED.options,
             channel_id = COALESCE(EXCLUDED.channel_id, accounts.channel_id)",
    )
    .bind(channel_id)
    .bind(&uid)
    .bind(&nickname)
    .bind(platform)
    .bind(&options)
    .execute(state.storage.pool())
    .await
    .map_err(|error| GatewayError::internal(format!("账号落库失败：{error}")))?;

    state
        .storage
        .set_credential(channel_id, &uid, Some(&credential))
        .await?;

    state.reload_accounts().await?;
    Ok(uid)
}

/// 任务快照：账号 × 任务清单 × 今日状态。
///
/// 这是 **N 次上游请求**（每账号一次 tasks + 一次 daily）。账号多时会慢，
/// 所以并发限 3、总超时 15s，超时的账号返回错误而不是让整个页面挂掉。
async fn task_live(State(state): State<SharedGateway>, Path(id): Path<String>) -> Response {
    let Some(adapter) = state.adapter_for_channel_async(&id).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "渠道不存在" }))).into_response();
    };

    let accounts = state.pool.by_channel(&id);
    let mut result = Vec::with_capacity(accounts.len());

    for account in accounts {
        let snapshot = account.snapshot();
        let running = state.task_progress(&id, &account.key.uid)["running"]
            .as_bool()
            .unwrap_or(false);

        if !account.enabled {
            result.push(json!({
                "uid": account.key.uid,
                "nickname": account.nickname,
                "enabled": false,
                "credentialOk": account.credentials().is_present(),
                "running": running,
                "runId": null,
                "error": null,
                "tasks": [],
                "daily": [],
            }));
            continue;
        }

        let tasks = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            adapter.list_tasks(&state.client, &snapshot),
        )
        .await
        {
            Ok(Ok(Some(list))) => list,
            Ok(Ok(None)) => Vec::new(),
            Ok(Err(error)) => {
                eprintln!(
                    "[gateway] 任务清单拉取失败 {}/{}：{}",
                    id, account.key.uid, error.message
                );
                Vec::new()
            }
            Err(_) => {
                eprintln!("[gateway] 任务清单拉取超时 {}/{}", id, account.key.uid);
                Vec::new()
            }
        };

        let daily = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            adapter.daily_actions(&state.client, &snapshot),
        )
        .await
        {
            Ok(Ok(Some(list))) => list,
            Ok(Ok(None)) => Vec::new(),
            Ok(Err(error)) => {
                eprintln!(
                    "[gateway] 每日动作拉取失败 {}/{}：{}",
                    id, account.key.uid, error.message
                );
                Vec::new()
            }
            Err(_) => {
                eprintln!("[gateway] 每日动作拉取超时 {}/{}", id, account.key.uid);
                Vec::new()
            }
        };

        /*
          成长体系快照：加油站能量 / 连登天数与补签卡 / 抽奖次数。

          单独一块而不是混进 daily：
            · daily 是"今天做没做"的动作，这些是**长期状态**（连登天数、剩余次数）；
            · 界面上它们属于"福利概览"，与任务列表并排，不是同一层信息。
        */
        let growth = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            crate::gateway::channels::workbuddy::growth_snapshot(&state.client, &snapshot),
        )
        .await;

        result.push(json!({
            "uid": account.key.uid,
            "nickname": account.nickname,
            "enabled": true,
            "credentialOk": account.credentials().is_present(),
            "running": running,
            "runId": null,
            "error": null,
            "tasks": tasks,
            "daily": daily,
            "growth": match growth {
                Ok(value) => json!({
                    "energy": value.energy,
                    "energyError": value.energy_error,
                    "streakDays": value.streak_days,
                    "streakError": value.streak_error,
                    "makeupCards": value.makeup_cards,
                    "tier7d": value.tier_7d,
                    "tier14d": value.tier_14d,
                    "tier28d": value.tier_28d,
                    "lotteryChances": value.lottery_chances,
                }),
                // 超时或异常：整块为空，界面显示"不可用"而不是一堆 0。
                Err(_) => serde_json::Value::Null,
            },
        }));
    }

    Json(json!({
        "channelId": id,
        "fetchedAt": now_ms(),
        "accounts": result,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct TaskRunBody {
    channel_id: String,
    uid: String,
    /// all | daily | tasks | checkin
    scope: String,
    /// 只执行这些任务码（`cat_travel` / `sign_in` / `create_canvas` …）。
    ///
    /// 为空则按 `scope` 跑整批。界面上的「执行此任务」按钮走这里 ——
    /// 没有它就只能整批跑，用户点"猫猫旅行"却触发了全部任务。
    #[serde(default)]
    codes: Vec<String>,
    /// Go 管理面预建的 `task_runs.id`。
    ///
    /// 有它才把明细与最终状态回写 PG；缺省时只走内存（排障/调试用）。
    run_id: Option<i64>,
}

/// 触发一次任务执行（后台跑，进度通过 `/internal/tasks/progress` 读）。
async fn task_run(State(state): State<SharedGateway>, Json(body): Json<TaskRunBody>) -> Response {
    let Some(adapter) = state.adapter_for_channel(&body.channel_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "渠道不存在" }))).into_response();
    };

    let Some(account) = state.account(&body.channel_id, &body.uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    if !state.begin_task_run(&body.channel_id, &body.uid) {
        return Json(json!({
            "started": false,
            "reason": "该账号的任务正在执行中",
        }))
        .into_response();
    }

    let shared = Arc::clone(&state);
    // 类型必须显式标注：`sender` 会移交给 `run_tasks`，`receiver` 在另一个
    // spawn 里被消费，编译器两边都推不出元素类型。
    let (sender, mut receiver) =
        tokio::sync::mpsc::unbounded_channel::<crate::gateway::channels::TaskRunResult>();

    // 动作间隔取自 `settings.task.daily_gap_ms`（SPEC 11.5）：界面上能调，
    // 硬编码就等于那个设置项是摆设。
    let daily_gap_ms = state.storage.load_policy().await.daily_gap_ms;

    // 进度消费者：把每条结果写进内存表，管理面轮询读到就能实时展示。
    //
    // 同时回写 `task_run_items`：内存表只服务"正在执行"的实时视图，
    // 历史页读的是 PG。两者写同一份结果，但**职责不同**，不能只留一个。
    let consumer_state = Arc::clone(&shared);
    let consumer_channel = body.channel_id.clone();
    let consumer_uid = body.uid.clone();
    let consumer_run_id = body.run_id;
    tokio::spawn(async move {
        while let Some(result) = receiver.recv().await {
            if let Some(run_id) = consumer_run_id {
                if let Err(error) = consumer_state
                    .storage
                    .append_task_item(
                        run_id,
                        &result.code,
                        &result.name,
                        // 任务清单里的项全是 `task`；`daily` / `checkin` 由
                        // 每日动作那条分支自己写，不走这个通道。
                        "task",
                        &result.status,
                        &result.message,
                        result.earned,
                    )
                    .await
                {
                    eprintln!("[gateway] 任务明细落库失败（run {run_id}）：{}", error.message);
                }
            }
            consumer_state.push_task_result(&consumer_channel, &consumer_uid, result);
        }
    });

    let runner_state = Arc::clone(&shared);
    let runner_channel = body.channel_id.clone();
    let runner_uid = body.uid.clone();
    let runner_run_id = body.run_id;
    let client = state.client.clone();
    let scope = body.scope.clone();
    /*
      请求指定的任务码集合。

      为什么需要它：`scope` 只能粗粒度选"每日动作 / 任务清单"，
      而界面上点的是**某一条具体任务**。没有这层过滤，
      点"猫猫旅行"会把 21 项任务全跑一遍 —— 既慢又浪费上游额度。
    */
    let wanted_codes: Vec<String> = body
        .codes
        .iter()
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();

    tokio::spawn(async move {
        let snapshot = account.snapshot();

        // 计数：最终状态按 SPEC 6.2 聚合。skipped 单独计，绝不计入 failed。
        let mut done = 0u32;
        let mut failed = 0u32;
        let mut skipped = 0u32;
        let mut earned = 0u32;
        let mut hard_error: Option<String> = None;

        // 顺序固定：每日动作在前，任务清单在后（SPEC 11.2）。
        // 每日动作是单次请求且时效性强（签到跨零点就漏了）。
        //
        // 每项之间检查取消标志（SPEC 11.7）：不打断正在飞的上游请求，
        // 但立刻停止后续项。已产生的结果与积分保留。
        if scope == "all" || scope == "daily" || scope == "checkin" {
            if let Ok(Some(actions)) = adapter.daily_actions(&client, &snapshot).await {
                for action in actions {
                    if runner_state.task_cancelled(&runner_channel, &runner_uid) {
                        break;
                    }
                    /*
                      指定了 codes 就只跑这些。

                      这条分支是「执行此任务」按钮的正确性来源：
                      没有它，点"猫猫旅行"会把全部任务跑一遍。
                    */
                    if !wanted_codes.is_empty() && !wanted_codes.iter().any(|code| code == &action.code) {
                        continue;
                    }
                    if action.done_today || !action.automatable {
                        /*
                          明确告诉用户"为什么没做"，而不是静默 continue。

                          静默跳过就是"点了没反应"的根源：猫猫旅行今天已完成，
                          点了之后既没动作也没提示，用户以为按钮坏了。
                        */
                        let reason = if action.done_today {
                            if action.note.is_empty() {
                                "今天已完成".to_string()
                            } else {
                                action.note.clone()
                            }
                        } else {
                            "该动作需要手工完成".to_string()
                        };
                        let result = crate::gateway::channels::TaskRunResult {
                            code: action.code.clone(),
                            name: action.name.clone(),
                            status: "skipped".into(),
                            message: reason,
                            earned: 0,
                        };
                        skipped += 1;
                        persist_item(runner_run_id, &runner_state, &result, kind_of(&action.code))
                            .await;
                        runner_state.push_task_result(&runner_channel, &runner_uid, result);
                        continue;
                    }
                    // traveling 跳过：重复 depart 会 400（SPEC 8.1③）。
                    if action.code == "cat_travel" && action.note.contains("旅行途中") {
                        let result = crate::gateway::channels::TaskRunResult {
                            code: action.code.clone(),
                            name: action.name.clone(),
                            status: "skipped".into(),
                            message: action.note.clone(),
                            earned: 0,
                        };
                        skipped += 1;
                        persist_item(runner_run_id, &runner_state, &result, kind_of(&action.code))
                            .await;
                        runner_state.push_task_result(&runner_channel, &runner_uid, result);
                        continue;
                    }
                    let outcome = adapter
                        .run_daily_action(&client, &snapshot, &action.code)
                        .await;
                    let result = match outcome {
                        Ok(result) => result,
                        Err(error) => crate::gateway::channels::TaskRunResult {
                            code: action.code.clone(),
                            name: action.name.clone(),
                            status: "failed".into(),
                            message: error.describe(),
                            earned: 0,
                        },
                    };

                    match result.status.as_str() {
                        "done" => done += 1,
                        "skipped" | "not_in_window" => skipped += 1,
                        _ => failed += 1,
                    }
                    earned += result.earned;
                    persist_item(runner_run_id, &runner_state, &result, kind_of(&action.code)).await;
                    runner_state.push_task_result(&runner_channel, &runner_uid, result);

                    tokio::time::sleep(std::time::Duration::from_millis(daily_gap_ms)).await;
                }
            }
        }

        let cancelled = runner_state.task_cancelled(&runner_channel, &runner_uid);
        if !cancelled && (scope == "all" || scope == "tasks") {
            match adapter.run_tasks(&client, &snapshot, &wanted_codes, sender).await {
                Ok(results) => {
                    // 逐条统计：`run_tasks` 的返回是**全量**结果，但前面推给
                    // 进度通道的也是同一批，所以这里不能用 mpsc 累加，否则翻倍。
                    done = results.iter().filter(|item| item.status == "done").count() as u32;
                    failed = results.iter().filter(|item| item.status == "failed").count() as u32;
                    skipped = results
                        .iter()
                        .filter(|item| item.status == "skipped" || item.status == "not_in_window")
                        .count() as u32;
                    earned = results.iter().map(|item| item.earned).sum();
                }
                Err(error) => {
                    hard_error = Some(error.message.clone());
                    runner_state.fail_task_run(&runner_channel, &runner_uid, &error.message);
                }
            }
        }

        // 落库：最终状态、计数、错误。
        //
        // 用户取消的 run 不在这里改状态 —— Go 侧已经把 status 改成 `cancelled`，
        // 而 SQL 带 `status = 'running'` 条件，所以取消后计数仍会写、状态保持
        // cancelled。这是刻意的：取消不是失败，也不该覆盖成 done。
        if let Some(run_id) = runner_run_id {
            if let Err(error) = runner_state
                .storage
                .finish_task_run(
                    run_id,
                    done,
                    failed,
                    skipped,
                    earned,
                    hard_error.as_deref(),
                )
                .await
            {
                eprintln!("[gateway] 任务记录落库失败（run {run_id}）：{}", error.message);
            }
        }

        if hard_error.is_none() {
            runner_state.finish_task_run(&runner_channel, &runner_uid, done, earned);
        }
    });

    Json(json!({ "started": true })).into_response()
}

/// 每日动作 / 签到 / 任务的 kind 判别。
///
/// `task_run_items.kind` 的 CHECK 约束只接受 `daily` / `task` / `checkin`，
/// 签到既是 daily action 又是独立概念，所以单独归档（面板按它分组）。
fn kind_of(code: &str) -> &'static str {
    match code {
        "sign_in" => "checkin",
        _ => "daily",
    }
}

/// 写一条每日动作的结果到 `task_run_items`（失败只打日志）。
async fn persist_item(
    run_id: Option<i64>,
    state: &SharedGateway,
    result: &crate::gateway::channels::TaskRunResult,
    kind: &str,
) {
    let Some(run_id) = run_id else {
        return;
    };
    if let Err(error) = state
        .storage
        .append_task_item(
            run_id,
            &result.code,
            &result.name,
            kind,
            &result.status,
            &result.message,
            result.earned,
        )
        .await
    {
        eprintln!("[gateway] 任务明细落库失败（run {run_id}）：{}", error.message);
    }
}

#[derive(Debug, Deserialize)]
struct ProgressQuery {
    channel_id: String,
    uid: String,
}

/// 请求取消某账号正在执行的任务（SPEC 11.7）。
///
/// 只置标志位：正在飞的上游请求不打断，后续项立刻停。
/// 已产生的结果与积分保留（积分已到账，不回滚）。
async fn task_cancel(State(state): State<SharedGateway>, Json(body): Json<CancelBody>) -> Response {
    let accepted = state.cancel_task_run(&body.channel_id, &body.uid);
    Json(json!({
        "accepted": accepted,
        "reason": if accepted { "" } else { "该账号当前没有执行中的任务" },
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct CancelBody {
    channel_id: String,
    uid: String,
}

/// 读任务执行进度。
async fn task_progress(
    State(state): State<SharedGateway>,
    Query(query): Query<ProgressQuery>,
) -> Response {
    Json(state.task_progress(&query.channel_id, &query.uid)).into_response()
}

#[derive(Debug, Deserialize)]
struct ClaimBody {
    channel_id: String,
    uid: String,
    code: String,
}

/// 单独领取一项已达标任务的奖励。
async fn task_claim(State(state): State<SharedGateway>, Json(body): Json<ClaimBody>) -> Response {
    let Some(adapter) = state.adapter_for_channel(&body.channel_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "渠道不存在" }))).into_response();
    };
    let Some(account) = state.account(&body.channel_id, &body.uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "账号不存在" }))).into_response();
    };

    match adapter
        .claim_task(&state.client, &account.snapshot(), &body.code)
        .await
    {
        Ok(result) => Json(json!(result)).into_response(),
        Err(error) => internal_error(error),
    }
}

/// 配置变更后通知网关重新加载渠道与账号。
async fn state_reload(State(state): State<SharedGateway>) -> Response {
    match state.reload_channels().await {
        Ok(()) => Json(json!({
            "ok": true,
            "channels": state.channels().len(),
            "accounts": state.pool.accounts().len(),
        }))
        .into_response(),
        Err(error) => internal_error(error),
    }
}
