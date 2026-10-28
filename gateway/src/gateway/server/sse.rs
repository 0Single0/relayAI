//! SSE 响应封装：把 mpsc 接收端变成 HTTP body 流。

use axum::{
    body::{Body, Bytes},
    http::{header, StatusCode},
    response::Response,
};
use futures_util::stream;
use tokio::sync::mpsc::Receiver;

/// `Body::from_stream` 需要 `Stream<Item = Result<T, E>>`；这里的接收端不可能出错，
/// 用 `Infallible` 表达"错误只走 SSE 内部帧"。
pub fn body_from_receiver(receiver: Receiver<Bytes>) -> Body {
    let stream = stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|item| (Ok::<Bytes, std::convert::Infallible>(item), receiver))
    });
    Body::from_stream(stream)
}

pub fn sse_response(receiver: Receiver<Bytes>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        // 反向代理下的缓冲开关；本机直连无害，但复制配置时能省一次排查。
        .header("x-accel-buffering", "no")
        .body(body_from_receiver(receiver))
        .expect("SSE 响应头是固定字面量")
}
