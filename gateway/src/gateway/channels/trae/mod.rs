//! Trae 渠道适配器（国内版 / 国际版 / 企业网关共用这一份代码）。
//!
//! ```json
//! { "id": "trae-cn", "adapter": "trae",
//!   "options": { "base_url": "https://trae-api-cn.mchost.guru" } }
//! ```
//!
//! **协议差异**（与 workbuddy / custom 两个渠道的本质不同）：
//! Trae 是两段式的 —— 先 `POST chat_sessions` 建会话拿 id，再
//! `GET /chat_sessions/{id}/events` 读流。所以它覆盖了
//! [`ChannelAdapter::chat_stream`] 的**默认实现**（默认实现只发一次 POST）。
//!
//! 解码也是**有状态**的：上游下发的是累计快照，需要按会话记住已发长度
//! （见 [`decode::Decoder`]）。因此每次请求都新建一个解码器，
//! 由返回的帧流闭包持有。

pub mod auth;
pub mod billing;
pub mod catalog;
pub mod decode;
pub mod encode;
pub mod identity;

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures_util::stream;
use serde_json::Value;

use crate::gateway::{
    channels::{
        AccountSnapshot, ChannelAdapter, Credential, LoginPoll, LoginTicket, OpsCaps,
        QuotaSnapshot, UpstreamModel,
    },
    errors::{ErrorKind, ErrorScope, GatewayError},
    protocol::cir::{CanonicalEvent, CanonicalRequest},
    transport::{self, FrameStream, UpstreamRequest},
};

pub struct TraeChannel {
    id: String,
    hosts: identity::Hosts,
    /// 单调序号：给日志/追踪用，避免同账号高并发下撞序号。
    sequence: AtomicU64,
    /// 原始模型条目缓存：`uid → (抓取时刻, config_name → 对象)`。
    ///
    /// 会话报文必须回传 `custom_model`，而每个请求都拉一次目录太浪费
    /// （目录接口几百 KB）。按账号缓存，TTL 内复用。
    model_objects: std::sync::Mutex<
        std::collections::HashMap<String, (u64, catalog::DetailMap)>,
    >,
}

/// `custom_model` 缓存的保鲜期：与矩阵目录的 TTL 对齐（5 分钟）。
const MODEL_OBJECTS_TTL_MS: u64 = 300_000;

impl TraeChannel {
    pub fn from_options(id: &str, options: &Value) -> Self {
        Self {
            id: id.to_string(),
            hosts: identity::Hosts::from_options(options),
            sequence: AtomicU64::new(0),
            model_objects: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 取某个账号的原始模型条目（带缓存）。
    ///
    /// 拿不到时返回空 map —— 调用方会退回"只发 model_name"的老行为，
    /// 那条路径上游可能报 995000，所以失败要如实打日志。
    async fn model_objects_for(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> catalog::DetailMap {
        let now = crate::gateway::timeutil::now_ms();
        if let Ok(cache) = self.model_objects.lock() {
            if let Some((fetched_at, map)) = cache.get(&account.uid) {
                if now.saturating_sub(*fetched_at) < MODEL_OBJECTS_TTL_MS && !map.is_empty() {
                    return map.clone();
                }
            }
        }
        let map = catalog::fetch_model_objects(client, &self.hosts, account).await;
        if map.is_empty() {
            eprintln!(
                "[trae] 模型条目拉取失败（账号 {}）：会话将只发 model_name，可能被上游拒为 995000",
                account.uid
            );
            return map;
        }
        if let Ok(mut cache) = self.model_objects.lock() {
            cache.insert(account.uid.clone(), (now, map.clone()));
        }
        map
    }

    /// 把路由/别名改写后的模型名换成上游真实拼写。
    ///
    /// 目录里怎么拼就怎么发 —— 上游按 `config_name` 大小写敏感匹配，
    /// 发小写名会得到 `4001 param invalid`（而不是"模型不存在"，很难排查）。
    fn upstream_model(&self, model: &str) -> String {
        identity::normalize_model(model)
    }

    /// 诊断：带上**这个账号的真实凭据**请求目录接口，原样返回上游响应。
    ///
    /// 为什么要有它：面板只显示"输出 —"/"档位 无"，看不出是上游没给、
    /// 字段名不同、还是接口直接失败。与其猜，不如把原始 JSON 摆出来。
    /// 它复用与真实调用**完全相同的地址和头**，所以看到的就是线上真实情况。
    pub async fn diagnose_catalog(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        which: &str,
    ) -> Result<serde_json::Value, GatewayError> {
        let (url, method) = match which {
            // 模型清单（决定有哪些模型 + multimodal）。
            "models" => (self.hosts.models_url(), "GET"),
            // 原生元数据（补 model_detail_list / 上下文 / 输出 / 档位）。
            "detail" => (self.hosts.detail_param_url(), "POST"),
            // 额度（顺带验证积分解析）。
            "credits" => (
                format!("{}/trae/api/v2/pay/ide_user_ent_usage", self.hosts.ug),
                "POST",
            ),
            other => {
                return Err(GatewayError::invalid_request(format!(
                    "未知的诊断目标 {other}（可用：models / detail / credits）"
                )))
            }
        };
        let headers = if which == "detail" {
            identity::detail_headers(account)
        } else {
            identity::base_headers(account, "application/json")
        };
        let body = match which {
            "detail" => serde_json::json!({
                "function": "solo_work_lite",
                "config_names": null,
                "need_prompt": false,
                "current_config_info": null,
                "poly_prompt": true,
                "mode_type": null,
                "agent_type": null,
            }),
            "credits" => serde_json::json!({"require_usage": true, "req_source": 1}),
            _ => serde_json::Value::Null,
        };

        let payload = if method == "GET" {
            transport::get_json(client, &url, &headers).await
        } else {
            transport::post_json(client, &url, &headers, &body).await
        };
        // 失败也照样把 URL 带回去 —— 404 与 401 的排查方向完全不同。
        match payload {
            Ok(value) => Ok(serde_json::json!({
                "target": which,
                "method": method,
                "url": url,
                "ok": true,
                "response": value,
                "top_level_keys": value
                    .as_object()
                    .map(|map| map.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            })),
            Err(error) => Ok(serde_json::json!({
                "target": which,
                "method": method,
                "url": url,
                "ok": false,
                "error": error.describe(),
            })),
        }
    }

    /// 建会话：拿到 `(session_id, message_id)`。
    async fn create_session(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<(String, String), GatewayError> {
        let model = self.upstream_model(&request.model);
        // **必须带上目录里的原始模型对象**：只发 model_name 时上游会
        // 静默换模型或直接回 995000（HTTP 200 里套一层业务错误）。
        let objects = self.model_objects_for(client, account).await;
        let custom_model = objects
            .get(&model)
            .or_else(|| objects.get(&model.to_ascii_lowercase()));
        let body = encode::build_session_body_with_model(
            &model,
            request,
            account,
            custom_model,
        );
        self.sequence.fetch_add(1, Ordering::Relaxed);
        let payload = transport::post_json(
            client,
            &self.hosts.chat_sessions_url(),
            &identity::base_headers(account, "application/json"),
            &body,
        )
        .await?;
        encode::parse_session_response(&payload)
    }
}

#[async_trait]
impl ChannelAdapter for TraeChannel {
    fn id(&self) -> &str {
        &self.id
    }

    fn adapter_kind(&self) -> &str {
        "trae"
    }

    /// Trae 只有签到（SPEC 11.1）。
    ///
    /// 它没有任务清单，`daily_actions` 走默认实现返回 `Ok(None)`，
    /// 只有 `sign_in` 被覆盖成真实请求。所以任务页对 Trae 会退化成
    /// 只有一张「签到」账号表 —— 这是预期行为，不是功能缺失。
    fn ops_caps(&self) -> OpsCaps {
        OpsCaps {
            tasks: false,
            daily: false,
            checkin: true,
        }
    }

    /// 只用于日志/追踪；真正的请求走 `chat_stream`（两段式）。
    fn build_chat_request(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
    ) -> Result<UpstreamRequest, GatewayError> {
        let model = self.upstream_model(&request.model);
        Ok(UpstreamRequest {
            url: self.hosts.chat_sessions_url(),
            headers: identity::base_headers(account, "application/json"),
            body: encode::build_session_body(&model, request, account),
        })
    }

    /// 一帧**中间帧** → CIR 事件（见 [`frames_to_events`]）。
    ///
    /// 注意：`chat_stream` 已经把上游帧翻译成中间帧了，所以这里解析的
    /// 不是 Trae 的原始 SSE，而是我们自己的带标签 JSON。原始帧的解码
    /// 在 `decode::Decoder` 里（有状态，需要按会话累计）。
    fn parse_event(&self, frame: &str) -> Vec<CanonicalEvent> {
        frames_to_events(frame)
    }

    fn classify_error(&self, status: u16, body: &str, _upstream_model: &str) -> GatewayError {
        // 上游绝大多数失败是 HTTP 200 + 业务码，所以先找业务码再按状态码兜底。
        if let Some(code) = extract_code(body) {
            if code != 0 {
                let message = extract_message(body);
                return compile_error(code, &message).with_upstream(body.to_string());
            }
        }
        crate::gateway::channels::openai_compat::classify_openai_error(status, body)
    }

    /// 两段式：建会话 → 读事件流。**覆盖默认实现**。
    async fn chat_stream(
        &self,
        account: &AccountSnapshot,
        request: &CanonicalRequest,
        client: &reqwest::Client,
    ) -> Result<FrameStream, GatewayError> {
        let (session_id, message_id) = self.create_session(client, account, request).await?;
        let url = self.hosts.events_url(&session_id, &message_id);
        let upstream = UpstreamRequest {
            url,
            headers: identity::base_headers(account, "text/event-stream"),
            body: Value::Null,
        };

        // 事件流是 GET，transport::send 只发 POST，所以这里自己发。
        let mut builder = client.get(&upstream.url);
        for (name, value) in &upstream.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await.map_err(GatewayError::from_reqwest)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(self.classify_error(status.as_u16(), &body, &request.model));
        }

        let decoder = std::sync::Arc::new(decode::Decoder::default());
        // **必须用保留整帧的版本**：Trae 的事件类型只在 `event:` 行里，
        // 剥掉之后 plan_item（正文）与 heartbeat（心跳）的 payload 无法区分。
        let frames = transport::frame_stream_full(response, EVENTS_IDLE_TIMEOUT);
        let decoder_for_stream = std::sync::Arc::clone(&decoder);

        // 把**上游帧**翻译成 CIR 事件再重新装帧：下游编码器吃的是
        // "每帧若干 CIR 事件"，而 Trae 一帧可能一个事件都没有（心跳/元数据），
        // 也可能一帧吐出多个（思考 + 正文 + 用量）。
        let translated = stream::unfold(
            (frames, decoder_for_stream, false),
            |(mut frames, decoder, mut done)| async move {
                use futures_util::StreamExt;
                loop {
                    if done {
                        return None;
                    }
                    match frames.next().await {
                        Some(Ok(frame)) => {
                            let events = decoder.decode(&frame);
                            if events.is_empty() {
                                continue;
                            }
                            if events
                                .iter()
                                .any(|event| matches!(event, CanonicalEvent::Finish { .. }))
                            {
                                done = true;
                            }
                            let payload = serde_json::to_string(&events_to_json(&events))
                                .unwrap_or_default();
                            return Some((Ok(payload), (frames, decoder, done)));
                        }
                        Some(Err(error)) => {
                            return Some((Err(error), (frames, decoder, true)));
                        }
                        None => {
                            // 上游没给 done 就断流：补一个 Finish，别让客户端一直等。
                            let tail = decoder.finish();
                            if tail.is_empty() {
                                return None;
                            }
                            let payload =
                                serde_json::to_string(&events_to_json(&tail)).unwrap_or_default();
                            return Some((Ok(payload), (frames, decoder, true)));
                        }
                    }
                }
            },
        );
        Ok(Box::pin(translated))
    }

    async fn list_models(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Vec<UpstreamModel>>, GatewayError> {
        match catalog::fetch(client, &self.hosts, account).await {
            Ok(Some(models)) => Ok(Some(catalog::merge(models))),
            // 拉取失败：没有可信的兜底清单可用（见 `catalog::SNAPSHOT` 的说明），
            // 如实回 `None` 表示"这次没拉到"，而不是伪装成"该渠道没有模型"。
            Ok(None) => Ok(None),
            // 凭据失效要抛出去触发刷新。
            Err(error) if error.kind == ErrorKind::AuthExpired => Err(error),
            Err(_) => Ok(None),
        }
    }

    async fn refresh(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<Credential>, GatewayError> {
        let Some(refresh_token) = account.refresh_token.as_deref().filter(|token| !token.is_empty())
        else {
            return Ok(None);
        };
        auth::refresh(client, &self.hosts, account, refresh_token)
            .await
            .map(Some)
    }

    async fn login_start(
        &self,
        _client: &reqwest::Client,
        platform: &str,
        // Trae 只有一个站点，没有国内/国际之分，realm 对它无意义。
        _realm: Option<&str>,
    ) -> Result<LoginTicket, GatewayError> {
        Ok(auth::login_start(platform))
    }

    async fn login_poll(
        &self,
        _client: &reqwest::Client,
        _state: &str,
    ) -> Result<LoginPoll, GatewayError> {
        Ok(auth::login_poll())
    }

    /// Trae 的授权回调是硬编码的回环地址，网关没有那个监听端口，
    /// 所以登录靠"用户把回调地址粘回来"完成（见 `login_complete`）。
    fn login_needs_callback(&self) -> bool {
        true
    }

    async fn login_complete(
        &self,
        client: &reqwest::Client,
        callback: &str,
    ) -> Result<Credential, GatewayError> {
        auth::credential_from_callback(client, &self.hosts, callback).await
    }

    async fn fetch_quota(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<Option<QuotaSnapshot>, GatewayError> {
        billing::fetch_quota(client, &self.hosts, account).await
    }

    async fn sign_in(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
    ) -> Result<bool, GatewayError> {
        Ok(billing::check_in(client, &self.hosts, account).await)
    }

    async fn diagnose(
        &self,
        client: &reqwest::Client,
        account: &AccountSnapshot,
        target: &str,
    ) -> Result<serde_json::Value, GatewayError> {
        self.diagnose_catalog(client, account, target).await
    }
}

/// 事件流空闲超时：Trae 会在模型思考时长时间不发帧，
/// 比通用渠道的 300 秒更宽松，否则长思考会被误判成断流。
const EVENTS_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// CIR 事件 → 可序列化的中间帧，好让帧流里只装"文本"。
///
/// 下游编码器吃的是 `parse_event` 的返回值，而 `FrameStream` 的元素是
/// `Result<String, _>`；为了在中间层保住有状态解码的结果，这里把事件
/// 序列化成 JSON 标签，再由 `parse_event` 转回来。
fn events_to_json(events: &[CanonicalEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|event| match event {
            CanonicalEvent::Text { text } => serde_json::json!({"t": "text", "v": text}),
            CanonicalEvent::Reasoning { text } => {
                serde_json::json!({"t": "reasoning", "v": text})
            }
            CanonicalEvent::ToolCall {
                index,
                id,
                name,
                arguments_delta,
            } => serde_json::json!({
                "t": "tool_call", "index": index, "id": id, "name": name, "args": arguments_delta
            }),
            CanonicalEvent::Usage(usage) => serde_json::json!({"t": "usage", "v": usage}),
            CanonicalEvent::Finish { reason } => serde_json::json!({"t": "finish", "v": reason}),
            CanonicalEvent::Error { message } => serde_json::json!({"t": "error", "v": message}),
        })
        .collect()
}

/// 中间帧 → CIR 事件。
pub fn frames_to_events(frame: &str) -> Vec<CanonicalEvent> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(frame) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let kind = item.get("t")?.as_str()?;
            Some(match kind {
                "text" => CanonicalEvent::Text {
                    text: item.get("v")?.as_str()?.to_string(),
                },
                "reasoning" => CanonicalEvent::Reasoning {
                    text: item.get("v")?.as_str()?.to_string(),
                },
                "tool_call" => CanonicalEvent::ToolCall {
                    index: item.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                    id: item.get("id").and_then(Value::as_str).map(str::to_string),
                    name: item.get("name").and_then(Value::as_str).map(str::to_string),
                    arguments_delta: item
                        .get("args")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
                "usage" => CanonicalEvent::Usage(
                    serde_json::from_value(item.get("v")?.clone()).unwrap_or_default(),
                ),
                "finish" => CanonicalEvent::Finish {
                    reason: item.get("v")?.as_str()?.to_string(),
                },
                "error" => CanonicalEvent::Error {
                    message: item.get("v")?.as_str()?.to_string(),
                },
                _ => return None,
            })
        })
        .collect()
}

fn extract_code(body: &str) -> Option<i64> {
    let value: Value = serde_json::from_str(body).ok()?;
    value
        .get("code")
        .or_else(|| value.get("error_code"))
        .and_then(Value::as_i64)
}

fn extract_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return body.to_string();
    };
    value
        .get("msg")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 业务码分类（与 `encode::business_error` 同一套语义，这里只补上下文）。
fn compile_error(code: i64, message: &str) -> GatewayError {
    let text = if message.trim().is_empty() {
        format!("Trae 上游返回 code={code}")
    } else {
        format!("{message}（code={code}）")
    };
    match code {
        401 | 1001 | 1002 | 1003 => GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            text,
            true,
        ),
        402 | 1005 => GatewayError::new(
            ErrorKind::QuotaExhausted,
            ErrorScope::Account,
            402,
            text,
            true,
        ),
        4023 | 404 => GatewayError::new(ErrorKind::ModelNotFound, ErrorScope::Request, 404, text, false),
        429 | 9074 => GatewayError::new(ErrorKind::RateLimited, ErrorScope::Account, 429, text, true),
        4001 | 400 | 422 => GatewayError::invalid_request(text),
        _ => GatewayError::upstream(502, text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn channel() -> TraeChannel {
        TraeChannel::from_options("trae-cn", &json!({}))
    }

    fn account() -> AccountSnapshot {
        AccountSnapshot {
            channel_id: "trae-cn".into(),
            uid: "u".into(),
            access_token: Some("jwt".into()),
            refresh_token: Some("rt".into()),
            enterprise_id: None,
            ..Default::default()
        }
    }

    fn request() -> CanonicalRequest {
        CanonicalRequest {
            model: "glm-5.3".into(),
            system: Vec::new(),
            messages: vec![crate::gateway::protocol::cir::Message {
                role: crate::gateway::protocol::cir::Role::User,
                content: vec![crate::gateway::protocol::cir::Block::text("hi")],
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                reasoning: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: true,
            reasoning: None,
            metadata: Default::default(),
            raw: json!({}),
        }
    }

    /// 面板靠这个标志切到"粘贴回调"界面。它必须是 true：
    /// Trae 的授权回调被硬编码成 `127.0.0.1:8765`，网关没有那个监听端口，
    /// 走轮询会永远停在"正在等待授权…"（真实踩过的坑）。
    #[test]
    fn login_requires_pasted_callback() {
        assert!(
            channel().login_needs_callback(),
            "Trae 不走轮询，必须让用户粘贴回调地址"
        );
    }

    #[test]
    fn channel_identity_comes_from_options() {
        let default = channel();
        assert_eq!(default.adapter_kind(), "trae");
        assert_eq!(default.hosts.remote, identity::DEFAULT_BASE_URL);

        let enterprise = TraeChannel::from_options(
            "trae-ent",
            &json!({"base_url": "https://trae.internal.corp"}),
        );
        assert_eq!(
            enterprise.hosts.remote,
            "https://trae.internal.corp/api/remote/v1"
        );
    }

    #[test]
    fn build_chat_request_targets_chat_sessions_with_cloud_ide_jwt() {
        let upstream = channel()
            .build_chat_request(&account(), &request())
            .expect("应当能构建报文");
        assert!(upstream.url.ends_with("/api/remote/v1/chat_sessions"));
        assert!(upstream
            .headers
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Cloud-IDE-JWT jwt"));
        assert_eq!(upstream.body["mode"], "code");
        assert_eq!(upstream.body["initial_message"]["model_name"], "glm-5.3");
    }

    #[test]
    fn business_codes_are_preferred_over_status_codes() {
        let classified = channel().classify_error(200, r#"{"code":1005,"msg":"quota"}"#, "glm-5.3");
        assert_eq!(classified.kind, ErrorKind::QuotaExhausted);
        // 上游原文必须留着，排障要看它。
        assert!(classified.upstream.as_deref().is_some_and(|raw| raw.contains("1005")));

        let model_gone = channel().classify_error(200, r#"{"code":4023,"msg":"unknown"}"#, "x");
        assert_eq!(model_gone.kind, ErrorKind::ModelNotFound);

        // 没有业务码：按状态码兜底。
        let plain = channel().classify_error(429, "too many requests", "x");
        assert_eq!(plain.kind, ErrorKind::RateLimited);
    }

    #[test]
    fn middle_frames_round_trip() {
        let events = vec![
            CanonicalEvent::Text { text: "你".into() },
            CanonicalEvent::Reasoning { text: "想".into() },
            CanonicalEvent::Finish { reason: "stop".into() },
        ];
        let frame = serde_json::to_string(&events_to_json(&events)).expect("序列化");
        let back = frames_to_events(&frame);
        assert_eq!(back.len(), 3);
        assert!(matches!(&back[0], CanonicalEvent::Text { text } if text == "你"));
        assert!(matches!(&back[1], CanonicalEvent::Reasoning { text } if text == "想"));
        assert!(matches!(&back[2], CanonicalEvent::Finish { reason } if reason == "stop"));
        // 非法帧返回空而不是 panic。
        assert!(frames_to_events("not json").is_empty());
    }

    /// 真实上游探针：用库里 trae-cn 账号对指定模型发一次真实会话，
    /// 验证不再回 `995000`。
    ///
    /// ```text
    /// cargo test --lib probe_trae_chat -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "需要真实数据库与 trae-cn 账号，手动执行"]
    async fn probe_trae_chat() {
        use crate::gateway::testkit;

        let model = std::env::var("PROBE_TRAE_MODEL")
            .unwrap_or_else(|_| "deepseek-v4.1-flash".into());

        let storage = testkit::open_storage().await;
        let account = testkit::account_snapshot(&storage, "trae-cn").await;

        let client = reqwest::Client::new();
        let channel = channel();

        // 先确认目录里拿得到这个模型的原始对象。
        let objects = channel.model_objects_for(&client, &account).await;
        println!("目录条目数：{}", objects.len());
        let Some(object) = objects.get(&model).or_else(|| objects.get(&model.to_ascii_lowercase()))
        else {
            panic!("目录里没有模型 {model}");
        };
        println!(
            "=== custom_model 全文 ===\n{}",
            serde_json::to_string_pretty(object).unwrap_or_default()
        );

        let request = CanonicalRequest {
            model: model.clone(),
            system: Vec::new(),
            messages: vec![crate::gateway::protocol::cir::Message {
                role: crate::gateway::protocol::cir::Role::User,
                content: vec![crate::gateway::protocol::cir::Block::text("只回复两个字：你好")],
                tool_calls: Vec::new(),
                tool_call_id: None,
                name: None,
                reasoning: None,
            }],
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            top_p: None,
            max_tokens: Some(64),
            stream: true,
            reasoning: None,
            metadata: Default::default(),
            raw: serde_json::json!({}),
        };

        // 走**真实链路** `chat_stream`：建会话 → 读事件流 → 解码成 CIR。
        use futures_util::StreamExt;
        match channel.chat_stream(&account, &request, &client).await {
            Ok(mut frames) => {
                let mut text = String::new();
                let mut reasoning = String::new();
                let mut sent_frames = 0usize;
                while let Some(frame) = frames.next().await {
                    sent_frames += 1;
                    match frame {
                        Ok(frame) => {
                            for event in channel.parse_event(&frame) {
                                match event {
                                    CanonicalEvent::Text { text: chunk } => text.push_str(&chunk),
                                    CanonicalEvent::Reasoning { text: chunk } => {
                                        reasoning.push_str(&chunk)
                                    }
                                    CanonicalEvent::Error { message } => {
                                        panic!("上游事件流报错：{message}")
                                    }
                                    CanonicalEvent::Finish { .. } => {}
                                    _ => {}
                                }
                            }
                        }
                        Err(error) => panic!("帧流失败：{}", error.describe()),
                    }
                    if text.chars().count() > 40 || sent_frames > 2000 {
                        break;
                    }
                }
                println!("=== 中间帧 {sent_frames} 个 ===");
                println!(
                    "=== 思考（前 120 字）===\n{}",
                    reasoning.chars().take(120).collect::<String>()
                );
                println!("=== 正文 ===\n{text}");
                assert!(!text.trim().is_empty(), "模型没有输出任何正文");
            }
            Err(error) => panic!("chat_stream 失败：{}", error.describe()),
        }
    }

    /// 端到端：上游 SSE（累计快照）→ 中间帧 → CIR 事件，正文不重复。
    #[test]
    fn upstream_frames_become_monotonic_cir_text() {
        let decoder = decode::Decoder::default();
        let mut collected = String::new();
        for raw in [
            "data: {\"response\":\"你\"}",
            "data: {\"response\":\"你好\"}",
            "data: {\"response\":\"你好，世界\"}",
        ] {
            let events = decoder.decode(raw);
            let frame = serde_json::to_string(&events_to_json(&events)).expect("序列化");
            for event in frames_to_events(&frame) {
                if let CanonicalEvent::Text { text } = event {
                    collected.push_str(&text);
                }
            }
        }
        assert_eq!(collected, "你好，世界", "累计快照必须只吐增量");
    }
}
