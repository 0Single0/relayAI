//! 上游传输：一次 HTTP 请求 + SSE 帧重组。
//!
//! 这一层不认识任何渠道，也不认识调度器：渠道适配器只提供报文，调度器只消费帧流。

use std::{pin::Pin, time::Duration};

use futures_util::{stream, Stream};

use crate::gateway::errors::GatewayError;

/// 上游帧流：每一项是一帧 SSE 的 `data` 负载（已去掉 `data:` 前缀与心跳）。
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<String, GatewayError>> + Send>>;

/// 上游请求报文，由渠道适配器构建（纯数据，便于单测）。
pub struct UpstreamRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

/// 上游失败：`status == 0` 表示没拿到 HTTP 状态码（连接/超时/断流）。
pub struct UpstreamFailure {
    pub status: u16,
    pub body: String,
}

/// 错误正文只留前 8KB：翻译错误码用不到更多，避免把大段 HTML 带进日志。
const ERROR_BODY_LIMIT: usize = 8 * 1024;

/// 截断上游正文到上限 —— **必须按 UTF-8 字符边界裁**。
///
/// `String::truncate` 在非字符边界上会 panic（中文正文超长时必现，
/// 表现为整个请求线程崩掉）。这里向前回退到最近的字符边界。
fn clip_body(mut text: String) -> String {
    if text.len() <= ERROR_BODY_LIMIT {
        return text;
    }
    let mut cut = ERROR_BODY_LIMIT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text
}

/// 一次性 JSON 请求（授权 / 刷新 / 目录这类非流式接口）。
pub async fn post_json(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
) -> Result<serde_json::Value, GatewayError> {
    let mut builder = client.post(url);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder.json(body).send().await.map_err(GatewayError::from_reqwest)?;
    decode_json_response(response).await
}

pub async fn get_json(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<serde_json::Value, GatewayError> {
    let mut builder = client.get(url);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await.map_err(GatewayError::from_reqwest)?;
    decode_json_response(response).await
}

async fn decode_json_response(response: reqwest::Response) -> Result<serde_json::Value, GatewayError> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // 只有**错误正文**才截断（限制日志体积）；
        // 成功响应必须完整 —— 任务清单/模型目录都可能远超 8KB，
        // 早期版本在这里一并截断，导致"EOF while parsing a string at column 8191"。
        return Err(GatewayError::upstream(status.as_u16(), clip_body(text)));
    }
    serde_json::from_str(&text).map_err(|error| {
        GatewayError::network(format!(
            "上游返回的不是合法 JSON（{} 字节）：{error}",
            text.len()
        ))
    })
}

pub async fn send(
    upstream: &UpstreamRequest,
    client: &reqwest::Client,
) -> Result<reqwest::Response, UpstreamFailure> {
    let mut builder = client.post(&upstream.url);
    for (name, value) in &upstream.headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .json(&upstream.body)
        .send()
        .await
        .map_err(|error| UpstreamFailure {
            status: 0,
            body: error.to_string(),
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = clip_body(response.text().await.unwrap_or_default());
    Err(UpstreamFailure {
        status: status.as_u16(),
        body,
    })
}

struct FrameState {
    response: reqwest::Response,
    buffer: Vec<u8>,
    done: bool,
}

/// 把上游响应体切成 SSE 帧流。
///
/// 两个细节：① 等分隔符到位再解 UTF-8，中文被切在 chunk 边界上也不会乱码
/// （复用 [`crate::sse::pop_sse_frame`]）；② 每帧读取都有空闲超时，避免半死连接占着账号槽。
pub fn frame_stream(response: reqwest::Response, idle_timeout: Duration) -> FrameStream {
    let state = FrameState {
        response,
        buffer: Vec::new(),
        done: false,
    };
    Box::pin(stream::unfold(state, move |mut state| async move {
        loop {
            match crate::sse::pop_sse_frame(&mut state.buffer) {
                Ok(Some(frame)) => {
                    if let Some(payload) = sse_payload(&frame) {
                        if payload != "[DONE]" {
                            return Some((Ok(payload), state));
                        }
                    }
                    continue;
                }
                Ok(None) => {}
                Err(message) => return Some((Err(GatewayError::network(message)), state)),
            }
            if state.done {
                // 收尾：缓冲区里剩的是不完整的一帧，丢弃（上游异常断流）。
                return None;
            }
            match tokio::time::timeout(idle_timeout, state.response.chunk()).await {
                Err(_) => {
                    return Some((
                        Err(GatewayError::network("上游流空闲超时（300 秒无数据）")),
                        state,
                    ))
                }
                Ok(Ok(Some(chunk))) => state.buffer.extend_from_slice(&chunk),
                Ok(Ok(None)) => state.done = true,
                Ok(Err(error)) => {
                    state.done = true;
                    return Some((Err(GatewayError::from_reqwest(error)), state));
                }
            }
        }
    }))
}

/// 把上游响应体切成 SSE 帧流，**保留整帧**（含 `event:` 行与 `id:` 行）。
///
/// 与 [`frame_stream`] 的差别只在"给不给事件名"：那个只吐 `data` 负载，
/// 对 OpenAI 兼容流够用；而 Trae 的事件类型**只存在于 `event:` 行**里
/// （同一份 payload 形状既可能是心跳也可能是正文），剥掉就无从判断。
pub fn frame_stream_full(response: reqwest::Response, idle_timeout: Duration) -> FrameStream {
    let state = FrameState {
        response,
        buffer: Vec::new(),
        done: false,
    };
    Box::pin(stream::unfold(state, move |mut state| async move {
        loop {
            match crate::sse::pop_sse_frame(&mut state.buffer) {
                Ok(Some(frame)) => {
                    // 纯心跳/注释帧没有 data 行，直接跳过（否则解码器白跑）。
                    if sse_payload(&frame).is_none() {
                        continue;
                    }
                    return Some((Ok(frame), state));
                }
                Ok(None) => {}
                Err(message) => return Some((Err(GatewayError::network(message)), state)),
            }
            if state.done {
                return None;
            }
            match tokio::time::timeout(idle_timeout, state.response.chunk()).await {
                Err(_) => {
                    return Some((
                        Err(GatewayError::network("上游流空闲超时（300 秒无数据）")),
                        state,
                    ))
                }
                Ok(Ok(Some(chunk))) => state.buffer.extend_from_slice(&chunk),
                Ok(Ok(None)) => state.done = true,
                Ok(Err(error)) => {
                    state.done = true;
                    return Some((Err(GatewayError::from_reqwest(error)), state));
                }
            }
        }
    }))
}

/// 从一帧 SSE 里取 `data` 负载（多行 data 用换行拼接）。心跳与注释行返回 None。
fn sse_payload(frame: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn body_clipping_respects_utf8_boundaries() {
        // 中文正文超长时截断不能 panic（曾导致整个请求线程崩）。
        let text = "中文内容".repeat(4000); // 48000 字节
        let clipped = super::clip_body(text);
        assert!(clipped.len() <= super::ERROR_BODY_LIMIT);
        // 仍是合法 UTF-8 且能取字符数（不 panic 即通过）。
        assert!(clipped.chars().count() > 0);
    }

    #[test]
    fn short_bodies_are_untouched() {
        assert_eq!(super::clip_body("短".to_string()), "短");
    }


    use super::*;

    #[test]
    fn payload_joins_multiline_data_and_skips_heartbeat() {
        assert_eq!(sse_payload(": ping\n\n"), None);
        assert_eq!(
            sse_payload("data: one\ndata: two\r\n\r\n").as_deref(),
            Some("one\ntwo")
        );
        assert_eq!(sse_payload("data: [DONE]\n\n").as_deref(), Some("[DONE]"));
    }
}
