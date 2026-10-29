//! Anthropic Messages 入口：`/v1/messages` 与 `/v1/messages/count_tokens`。
//!
//! 只做协议转换：解码 → CIR →（渠道层改写）→ 上游；回来时把 CIR 事件编码成
//! Anthropic 的事件序列。错误信封按 Anthropic 形状返回。

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
    core::{dispatch, usage as estimate},
    errors::GatewayError,
    keyauth,
    protocol::{
        cir::CanonicalEvent,
        decode,
        encode::anthropic::{self, StreamEncoder},
    },
    server::{
        openai::{header_session, record_usage, CompletionMeta},
        sse,
    },
    state::SharedGateway,
};

pub async fn messages(
    State(state): State<SharedGateway>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 先留一份句柄：state 会移进 run，错误分支要拿它记录拒绝原因。
    let recorder = Arc::clone(&state);
    match run(state, headers, body).await {
        Ok(response) => response,
        Err(error) => {
            crate::gateway::server::record_rejection(&recorder, "/v1/messages", &error);
            error.into_response()
        },
    }
}

/// token 估算：上游没有 Anthropic 分词器，用本地 CJK 感知的口径保证数量级正确。
pub async fn count_tokens(
    State(state): State<SharedGateway>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 鉴权要先 await（要查库），所以不能塞进同步闭包里。
    if let Err(error) = keyauth::resolve(&state, &headers).await {
        return error.into_anthropic_response();
    }

    let result = (|| -> Result<Response, GatewayError> {
        let body: Value = serde_json::from_slice(&body)
            .map_err(|error| GatewayError::invalid_request(format!("请求体不是合法 JSON：{error}")))?;
        let request = decode::anthropic::decode_messages(&body)?;
        Ok(Json(json!({
            "input_tokens": estimate::estimate_request_tokens(&request)
        }))
        .into_response())
    })();

    match result {
        Ok(response) => response,
        Err(error) => error.into_anthropic_response(),
    }
}

async fn run(
    state: SharedGateway,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, GatewayError> {
    let key = keyauth::resolve(&state, &headers).await?;

    let permit = Arc::clone(&state.slots)
        .try_acquire_owned()
        .map_err(|_| GatewayError::overloaded())?;

    let body: Value = serde_json::from_slice(&body)
        .map_err(|error| GatewayError::invalid_request(format!("请求体不是合法 JSON：{error}")))?;
    let mut request = decode::anthropic::decode_messages(&body)?;
    // Anthropic 客户端把会话线索放在 metadata.user_id 里，请求头也可能带。
    if let Some(session) = header_session(&headers) {
        request.metadata.header_session = Some(session);
    }

    let streaming = request.stream;
    let model = request.model.clone();
    let effort = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.clone());
    let input_tokens = estimate::estimate_request_tokens(&request);
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

    if let Some(key_id) = key.key_id {
        state.storage.touch_api_key(key_id);
    }

    let meta = CompletionMeta {
        model: model.clone(),
        effort,
        prompt_tokens: input_tokens,
        stream: streaming,
        started,
        key_id: key.key_id,
        request_payload: super::payload::request_json(&state, &request).await,
    };

    if streaming {
        Ok(stream_response(
            Arc::clone(&state),
            session,
            meta,
            permit,
        ))
    } else {
        let result = collect_message(Arc::clone(&state), session, &meta).await;
        drop(permit);
        result
    }
}

fn stream_response(
    state: SharedGateway,
    session: dispatch::UpstreamSession,
    meta: CompletionMeta,
    permit: OwnedSemaphorePermit,
) -> Response {
    let (sender, receiver) = mpsc::channel::<Bytes>(64);
    let message_id = format!("msg_{}", Uuid::new_v4().simple());

    tokio::spawn(async move {
        let _permit = permit;
        let mut session = session;
        let mut encoder = StreamEncoder::new(message_id, meta.model.clone(), meta.prompt_tokens);
        let mut ttft_ms: Option<u64> = None;
        let mut produced = String::new();
        let mut finish = "stop".to_string();
        let mut status = "completed";
        let mut error_detail: Option<String> = None;
        // 上游会在最后几个 chunk 里给 usage（含积分）：必须留下来，
        // 否则这一行会退化成"本地估算"，积分也一起丢掉。
        let mut usage = crate::gateway::protocol::cir::Usage::default();
        let mut reported_usage = false;

        for frame in encoder.start() {
            if sender.send(frame).await.is_err() {
                return;
            }
        }

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
                    CanonicalEvent::Usage(value) => {
                        usage = value.clone();
                        reported_usage = true;
                    }
                    CanonicalEvent::Error { message } => {
                        status = "upstream_aborted";
                        stop = true;
                        error_detail.get_or_insert_with(|| message.clone());
                    }
                    CanonicalEvent::Finish { reason } => finish = reason.clone(),
                    CanonicalEvent::Text { text } | CanonicalEvent::Reasoning { text } => {
                        if ttft_ms.is_none() {
                            ttft_ms = Some(meta.started.elapsed().as_millis() as u64);
                        }
                        produced.push_str(text);
                    }
                    _ => {}
                }
                for out in encoder.push(event) {
                    if sender.send(out).await.is_err() {
                        status = "client_aborted";
                        break 'stream;
                    }
                }
            }
            if stop {
                break;
            }
        }

        for out in encoder.finish(&finish) {
            let _ = sender.send(out).await;
        }
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

async fn collect_message(
    state: SharedGateway,
    mut session: dispatch::UpstreamSession,
    meta: &CompletionMeta,
) -> Result<Response, GatewayError> {
    let message_id = format!("msg_{}", Uuid::new_v4().simple());
    let mut events = Vec::new();
    let mut ttft_ms: Option<u64> = None;
    let mut error_detail = None;
    loop {
        let next = session.frames.next().await;
        let Some(frame) = next else { break };
        let batch = match frame {
            Ok(frame) => session.adapter.parse_event(&frame),
            Err(error) => {
                error_detail.get_or_insert_with(|| error.describe());
                vec![CanonicalEvent::Error {
                    message: error.message,
                }]
            }
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

    let message = anthropic::fold(&events, &message_id, &meta.model);
    let produced = match &message {
        Ok(value) => value["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .unwrap_or_default(),
        Err(_) => String::new(),
    };
    let (usage, reported) = events
        .iter()
        .rev()
        .find_map(|event| match event {
            CanonicalEvent::Usage(usage) => Some((usage.clone(), true)),
            _ => None,
        })
        .unwrap_or_default();
    let error_detail = error_detail.or_else(|| message.as_ref().err().map(|error| error.describe()));
    record_usage(
        &state,
        &session,
        meta,
        if message.is_ok() { "completed" } else { "failed" },
        reported,
        usage,
        &produced,
        ttft_ms,
        error_detail,
    );
    Ok(Json(message?).into_response())
}
