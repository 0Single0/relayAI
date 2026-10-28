//! OpenAI 兼容入口：Chat Completions（含旧版 completions）与模型清单。
//!
//! 入口只做四件事：鉴权、协议 ↔ CIR、把事件编码回 SSE/JSON、记一条用量。
//! 选号、冷却、重试、凭据刷新都在 `core` 里，入口不感知渠道。

use std::{sync::Arc, time::Instant};

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use uuid::Uuid;

use crate::gateway::{
    core::{
        dispatch::{self, UpstreamSession},
        usage as estimate,
    },
    errors::GatewayError,
    keyauth,
    protocol::{
        cir::{CanonicalEvent, Usage},
        decode, encode,
    },
    server::sse,
    state::SharedGateway,
    storage::{UsagePayload, UsageRecord},
    timeutil::now_ms,
};

pub async fn chat_completions(
    State(state): State<SharedGateway>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let recorder = Arc::clone(&state);
    match run_chat(state, headers, body).await {
        Ok(response) => response,
        Err(error) => {
            crate::gateway::server::record_rejection(&recorder, "/v1/chat/completions", &error);
            error.into_response()
        }
    }
}

async fn run_chat(
    state: SharedGateway,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, GatewayError> {
    // 鉴权返回完整上下文（含两个 allowlist），不再只是一个布尔结果。
    let key = keyauth::resolve(&state, &headers).await?;

    // 全局并发闸门：满载直接 503。permit 活到流结束。
    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| GatewayError::overloaded())?;

    let body: Value = serde_json::from_slice(&body)
        .map_err(|error| GatewayError::invalid_request(format!("请求体不是合法 JSON：{error}")))?;
    let mut request = decode::openai::decode_chat(&body)?;
    request.metadata.header_session = keyauth::header_session(&headers);

    let streaming = request.stream;
    let model = request.model.clone();
    let effort = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.clone());
    let prompt_tokens = estimate::estimate_request_tokens(&request);
    let started = Instant::now();

    let session = match dispatch::open_chat_session(&state, &request, &key).await {
        Ok(session) => session,
        Err(error) => {
            state
                .stats
                .failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(error);
        }
    };
    state
        .stats
        .requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // 记录"这个 Key 用过"，管理面的「最近使用」靠它。
    if let Some(key_id) = key.key_id {
        state.storage.touch_api_key(key_id);
    }

    let finish = CompletionMeta {
        model,
        effort,
        prompt_tokens,
        stream: streaming,
        started,
        key_id: key.key_id,
        // 内容留存默认关闭（隐私敏感）。打开时把请求侧的语义内容序列化下来。
        request_payload: super::payload::request_json(&state, &request).await,
    };
    if streaming {
        Ok(stream_response(
            Arc::clone(&state),
            session,
            finish,
            permit,
        ))
    } else {
        let result = collect_completion(Arc::clone(&state), session, &finish).await;
        drop(permit);
        result.map(|completion| Json(completion).into_response())
    }
}

pub async fn list_models(
    State(state): State<SharedGateway>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    let key = keyauth::resolve(&state, &headers).await?;

    // 清单来自矩阵：它已经把各渠道目录合并、清洗、按对外顺序排好。
    let matrix = state.ensure_matrix(false).await;
    let created = now_ms() / 1000;

    let data: Vec<Value> = matrix
        .public_models()
        .into_iter()
        // 按 Key 过滤（SPEC 12.1 第 2 条）。
        //
        // 不过滤的后果：客户端看到 20 个模型、实际只有 8 个能用，
        // 用户会以为平台坏了。这是最容易漏的一处，因为它不影响调度逻辑。
        .filter(|id| key.allows_model(id))
        .filter(|id| {
            // 用 any 而不是 all：只要还有一条本 Key 能走的路由，模型就该出现。
            matrix
                .routes(id)
                .iter()
                .any(|route| key.allows_channel(&route.channel_id))
        })
        .map(|id| {
            let routes = matrix.routes(&id);
            // 元数据取信息量最大的一条候选（各候选的上游名/上限可能不同，取排序最优的）。
            let best = routes.first();
            json!({
                "id": id,
                "object": "model",
                "created": created,
                "owned_by": best
                    .map(|route| route.channel_id.clone())
                    .unwrap_or_default(),
                // 额外字段：这条模型能走哪些渠道（客户端会忽略，面板/脚本有用）。
                "channels": routes
                    .iter()
                    .map(|route| route.channel_id.clone())
                    .collect::<Vec<_>>(),
                "free": routes.iter().any(|route| route.is_free()),
                // 元数据（OpenAI 官方字段之外的扩展，客户端会忽略）：
                // 没有 /v1/models 端点的供应商（包括我们自己）不带这些时，
                // 下游的"获取模型"就只能拿到名字，上下文/输出全部显示未知。
                "context_length": best.and_then(|route| route.limits.context),
                "max_output_tokens": best.and_then(|route| route.limits.max_output),
                "capabilities": best
                    .map(|route| json!({
                        "vision": route.capabilities.vision,
                        "tool_calls": route.capabilities.tool_calls,
                        "reasoning": route.capabilities.reasoning,
                    }))
                    .unwrap_or(json!({})),
                "reasoning_efforts": best.map(|route| route.reasoning_efforts.clone())
                    .unwrap_or_default(),
                "reasoning_default_effort": best.and_then(|route| route.reasoning_default_effort.clone()),
                "always_reasoning": best.map(|route| route.always_reasoning).unwrap_or(false),
            })
        })
        .collect();
    Ok(Json(json!({ "object": "list", "data": data })).into_response())
}

/// 一次请求里与用量记账有关的固定信息。
pub(super) struct CompletionMeta {
    pub(super) model: String,
    pub(super) effort: Option<String>,
    /// 请求的输入 token 估算值（上游不给 usage 时用）。
    pub(super) prompt_tokens: u64,
    pub(super) stream: bool,
    pub(super) started: Instant,
    /// 发起请求的 Key；免鉴权模式为 None。
    ///
    /// 落进 usage_logs 才能回答"这个 Key 烧了哪个渠道多少额度"（SPEC 5.5）。
    pub(super) key_id: Option<i64>,
    /// 请求侧的脱敏内容（只在 `payload.enabled` 打开时填）。
    pub(super) request_payload: Option<String>,
}

/// 流式：把上游帧流按渠道解析成 CIR 事件，再编码成 OpenAI SSE，结束时记一条用量。
fn stream_response(
    state: SharedGateway,
    session: UpstreamSession,
    meta: CompletionMeta,
    permit: OwnedSemaphorePermit,
) -> Response {
    let (sender, receiver) = mpsc::channel::<Bytes>(64);
    let id = format!("chatcmpl-{}", Uuid::new_v4().simple());
    let created = now_ms() / 1000;

    tokio::spawn(async move {
        // permit 与 session 活到任务结束：客户端断连导致 send 失败时一起回收。
        let _permit = permit;
        let mut session = session;
        let mut usage = Usage::default();
        let mut reported_usage = false;
        let mut ttft_ms: Option<u64> = None;
        let mut produced = String::new();
        let mut status = "completed";
        let mut error_detail: Option<String> = None;

        'stream: loop {
            let next = session.frames.next().await;
            let Some(frame) = next else { break };
            let events = match frame {
                Ok(frame) => session.adapter.parse_event(&frame),
                Err(error) => {
                    status = "upstream_aborted";
                    error_detail.get_or_insert_with(|| error.describe());
                    vec![CanonicalEvent::Error {
                        message: error.message,
                    }]
                }
            };
            let mut stop = false;
            for event in &events {
                match event {
                    CanonicalEvent::Error { message } => {
                        stop = true;
                        error_detail.get_or_insert_with(|| message.clone());
                    }
                    CanonicalEvent::Text { text } | CanonicalEvent::Reasoning { text } => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(meta.started.elapsed().as_millis() as u64);
                        }
                        produced.push_str(text);
                    }
                    CanonicalEvent::Usage(value) => {
                        usage = value.clone();
                        reported_usage = true;
                    }
                    _ => {}
                }
                if let Some(chunk) = encode::openai::chunk(event, &id, &meta.model, created) {
                    if sender.send(encode::openai::sse_data(&chunk)).await.is_err() {
                        status = "client_aborted";
                        break 'stream;
                    }
                }
            }
            if stop {
                break;
            }
        }
        let _ = sender.send(encode::openai::sse_done()).await;

        record_usage(
            &state,
            &session,
            &meta,
            status,
            reported_usage,
            usage,
            &produced,
            ttft_ms,
            error_detail,
        );
    });

    sse::sse_response(receiver)
}

/// 非流式：把整条事件流折叠成一个 `chat.completion`。
async fn collect_completion(
    state: SharedGateway,
    mut session: UpstreamSession,
    meta: &CompletionMeta,
) -> Result<Value, GatewayError> {
    let id = format!("chatcmpl-{}", Uuid::new_v4().simple());
    let created = now_ms() / 1000;
    let mut events = Vec::new();
    let mut ttft_ms: Option<u64> = None;
    loop {
        let next = session.frames.next().await;
        let Some(frame) = next else { break };
        let batch = match frame {
            Ok(frame) => session.adapter.parse_event(&frame),
            Err(error) => vec![CanonicalEvent::Error {
                message: error.message,
            }],
        };
        for event in &batch {
            if ttft_ms.is_none()
                && matches!(
                    event,
                    CanonicalEvent::Text { .. } | CanonicalEvent::Reasoning { .. }
                )
            {
                ttft_ms = Some(meta.started.elapsed().as_millis() as u64);
            }
        }
        events.extend(batch);
    }

    let completion = encode::openai::fold(&events, &id, &meta.model, created);
    let error_detail = completion.as_ref().err().map(|error| error.describe());
    let (status, usage, reported, produced) = match &completion {
        Ok(value) => {
            let text = value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (usage, reported) = usage_from_events(&events);
            ("completed", usage, reported, text)
        }
        Err(_) => {
            let (usage, reported) = usage_from_events(&events);
            ("failed", usage, reported, String::new())
        }
    };
    record_usage(
        &state, &session, meta, status, reported, usage, &produced, ttft_ms, error_detail,
    );
    completion
}

fn usage_from_events(events: &[CanonicalEvent]) -> (Usage, bool) {
    for event in events.iter().rev() {
        if let CanonicalEvent::Usage(usage) = event {
            return (usage.clone(), true);
        }
    }
    (Usage::default(), false)
}

/// 记一条用量：拿到上游 usage 就用它，拿不到就本地估算并标记。
pub(super) fn record_usage(
    state: &SharedGateway,
    session: &UpstreamSession,
    meta: &CompletionMeta,
    status: &str,
    reported_usage: bool,
    usage: Usage,
    produced: &str,
    ttft_ms: Option<u64>,
    error_detail: Option<String>,
) {
    let usage = if reported_usage {
        usage
    } else {
        estimate::estimate_completion(meta.prompt_tokens, produced)
    };
    // 内容留存（默认关闭）：把请求与响应一起存下来，便于事后复现。
    // 只存 messages / 最终文本这类语义内容，不存原始 HTTP 体
    // （里面有客户端自己的 header，可能含认证信息）。
    let payload = meta.request_payload.as_ref().map(|request| UsagePayload {
        request: request.clone(),
        response: serde_json::json!({ "text": produced }).to_string(),
    });

    /*
        计价。

        按**上游模型名**查价（不是对外名）：同一个对外模型在不同渠道可能映射到
        不同的上游模型，而价格是上游模型决定的。
        渠道倍率按 channel_id 查 —— 它和价格是两件事，不能混。

        找不到价格时成本为 0 而不是报错：用户要的是回复，不是账单。
        但这样的行会留下 `upstream_cost = 0`，运维页据此能发现"有模型没配价"。
    */
    let channel_id = session.account().key.channel_id.clone();
    let billing = crate::gateway::core::billing::compute(
        state.price_book().lookup(&session.selection.route.upstream_model),
        crate::gateway::core::billing::BillableTokens {
            prompt: usage.prompt_tokens,
            completion: usage.completion_tokens,
            cached: usage.cached_tokens,
        },
        // 模型倍率暂未单独配置，先用 1.0；渠道倍率从渠道表读。
        1.0,
        state.channel_ratio(&channel_id),
    );

    state.storage.record_usage(UsageRecord {
        request_id: session.trace_id.clone(),
        key_id: meta.key_id,
        model: meta.model.clone(),
        upstream_model: Some(session.selection.route.upstream_model.clone()),
        channel_id,
        account_uid: session.account().key.uid.clone(),
        protocol: "chat".into(),
        status: status.to_string(),
        is_rejection: false,
        stream: meta.stream,
        attempts: session.attempts,
        usage,
        estimated: !reported_usage,
        ttft_ms,
        total_ms: meta.started.elapsed().as_millis() as u64,
        effort: meta.effort.clone(),
        error_detail,
        upstream_cost: billing.upstream_cost,
        billed_cost: billing.billed_cost,
        price_snapshot: billing.snapshot,
        payload,
    });
    eprintln!(
        "[gateway] trace={} done account={} upstream={} via_affinity={} affinity_key={:?} attempts={} status={status} ttft={:?}ms estimated={}",
        session.trace_id,
        session.account().key,
        session.selection.route.upstream_model,
        session.selection.via_affinity,
        session.affinity_key,
        session.attempts,
        ttft_ms,
        !reported_usage
    );
}

// 鉴权与请求头解析已移到 `keyauth` 模块（SPEC 12）：
// 那里返回完整的 KeyContext（含 channel_allow / model_allow），
// 而不只是一个布尔结果。
pub(super) use crate::gateway::keyauth::header_session;
