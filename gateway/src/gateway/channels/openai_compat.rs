//! OpenAI 兼容渠道的通用部分：状态码语义、限流 reset 解析、SSE 帧 → CIR 事件。
//!
//! workbuddy 这类"协议本身就是 OpenAI 形状、只是加了私有错误码"的渠道共用这一份。

use serde_json::Value;

use crate::gateway::errors::{ErrorKind, ErrorScope, GatewayError};
use crate::gateway::protocol::cir::{CanonicalEvent, Usage};
use crate::gateway::timeutil::civil_to_epoch_ms;

/// 状态码 → 统一语义。调用方（调度器）在拿到结果后会把带 reset 的 429 收窄到模型级冷却。
pub fn classify_openai_error(status: u16, body: &str) -> GatewayError {
    // 分类结果带上上游原文：面板的请求日志、探针、错误详情全靠它。
    classify_status(status, body).with_upstream(raw(body))
}

/// 上游原文保留换行，最多 16 KiB；600 字节会截掉本地化说明和申诉提示。
pub fn raw(body: &str) -> String {
    const MAX_BYTES: usize = 16 * 1024;
    let trimmed = body.trim();
    if trimmed.len() <= MAX_BYTES {
        return trimmed.to_string();
    }
    let mut cut = MAX_BYTES;
    while cut > 0 && !trimmed.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n…（上游响应超过 16 KiB，已截断）", &trimmed[..cut])
}

fn classify_status(status: u16, body: &str) -> GatewayError {
    match status {
        400 | 413 | 422 => GatewayError::invalid_request(short(body, "上游拒绝了请求参数")),
        401 => GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            short(body, "凭据失效"),
            true,
        ),
        402 => GatewayError::new(
            ErrorKind::QuotaExhausted,
            ErrorScope::Account,
            402,
            short(body, "余额或额度不足"),
            true,
        ),
        403 => GatewayError::new(
            ErrorKind::ContentPolicy,
            ErrorScope::Request,
            403,
            short(body, "上游内容审核拒绝"),
            false,
        ),
        404 => GatewayError::new(
            ErrorKind::ModelNotFound,
            ErrorScope::Request,
            404,
            short(body, "上游没有这个模型"),
            false,
        ),
        408 | 409 | 425 | 429 => GatewayError::new(
            ErrorKind::RateLimited,
            ErrorScope::Account,
            429,
            short(body, "上游限流"),
            true,
        )
        .with_reset_at(parse_rate_limit_reset(body)),
        500..=599 => GatewayError::upstream(status, short(body, "上游服务异常")),
        // 其余 4xx 当作请求问题：换号也解决不了，别浪费额度。
        _ => GatewayError::invalid_request(short(body, &format!("上游返回 {status}"))),
    }
}

/// 一帧 OpenAI 形状的 SSE → CIR 事件。
///
/// 上游会在 delta 里塞空占位（空 `reasoning_content`、空 `tool_calls`、
/// `{"name":"","arguments":""}` 的 function_call）；空串一律不产生事件，
/// 客户端就不会渲染出空内容。
pub fn parse_openai_chunk(frame: &str) -> Vec<CanonicalEvent> {
    let Ok(value) = serde_json::from_str::<Value>(frame) else {
        return Vec::new();
    };
    // HTTP 200 的 SSE 也可能携带错误信封；不能把它当空帧忽略并记成成功。
    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
        let reason = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("上游返回流内错误");
        return vec![CanonicalEvent::Error {
            message: format!(
                "{}；上游返回：{}",
                short(reason, "上游返回流内错误"),
                raw(frame)
            ),
        }];
    }
    let mut events = Vec::new();

    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        if let Some(usage) = parse_usage(usage) {
            events.push(CanonicalEvent::Usage(usage));
        }
    }

    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for choice in choices {
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            events.push(CanonicalEvent::Text {
                text: text.to_string(),
            });
        }
        if let Some(text) = delta
            .get("reasoning_content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            events.push(CanonicalEvent::Reasoning {
                text: text.to_string(),
            });
        }
        for (index, call) in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .filter(|args| !args.is_empty())
                .map(str::to_string);
            if name.is_none() && arguments.is_none() && call.get("id").is_none() {
                continue;
            }
            events.push(CanonicalEvent::ToolCall {
                index: call
                    .get("index")
                    .and_then(Value::as_u64)
                    .unwrap_or(index as u64) as u32,
                id: call.get("id").and_then(Value::as_str).map(str::to_string),
                name,
                arguments_delta: arguments,
            });
        }
        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            events.push(CanonicalEvent::Finish {
                reason: reason.to_string(),
            });
        }
    }

    events
}

/// 上游 usage → 内部用量：缓存字段按别名优先级挑一个，其余直接对应。
pub fn parse_usage(usage: &Value) -> Option<Usage> {
    if usage.is_null() {
        return None;
    }
    let number = |path: &[&str]| -> u64 {
        let mut current = usage;
        for key in path {
            match current.get(*key) {
                Some(next) => current = next,
                None => return 0,
            }
        }
        current.as_u64().unwrap_or(0)
    };
    let cached = {
        let aliases = [
            vec!["prompt_tokens_details", "cached_tokens"],
            vec!["prompt_cache_hit_tokens"],
            vec!["cache_read_input_tokens"],
            vec!["cached_tokens"],
            vec!["input_tokens_details", "cached_tokens"],
        ];
        aliases
            .iter()
            .map(|path| number(path))
            .find(|value| *value > 0)
            .unwrap_or(0)
    };
    Some(Usage {
        prompt_tokens: number(&["prompt_tokens"]),
        completion_tokens: number(&["completion_tokens"]),
        reasoning_tokens: number(&["completion_tokens_details", "reasoning_tokens"]),
        cached_tokens: cached,
        total_tokens: number(&["total_tokens"]),
        credit: usage.get("credit").and_then(Value::as_f64),
    })
}

/// 把上游错误正文压成一行提示；正文为空时用兜底文案。
pub fn short(body: &str, fallback: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let mut text = trimmed.replace(['\r', '\n'], " ");
    if text.len() > 400 {
        let mut cut = 400;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

/// 从 429 正文里抽「恢复时刻」：
/// `... your usage will reset at 2026-09-19 18:29:03 UTC+8 ...`
///
/// 抽到 = 只冷该模型；抽不到 = 账号级软限流（调度器据此分叉）。
pub fn parse_rate_limit_reset(body: &str) -> Option<u64> {
    // **不认措辞、只认时间戳**：上游会按 `Accept-Language` 本地化错误文案 ——
    // 英文是 "your usage will reset at 2026-09-19 18:29:03 UTC+8"，
    // 中文是 "将在 2026-10-11 14:46:40 UTC+8 重置，您也可以切换其他模型继续使用"。
    // 早先只找英文 "reset at"，中文文案就解析不出来 → 退化成"恢复时刻未知"，
    // 于是按 10m/20m/40m 退避重试，而真实恢复要等到第二天（差了一整天）。
    let start = find_timestamp(body)?;
    let stamp = body.get(start..start + 19)?;
    let year: i64 = stamp.get(0..4)?.parse().ok()?;
    let month: i64 = stamp.get(5..7)?.parse().ok()?;
    let day: i64 = stamp.get(8..10)?.parse().ok()?;
    let hour: i64 = stamp.get(11..13)?.parse().ok()?;
    let minute: i64 = stamp.get(14..16)?.parse().ok()?;
    let second: i64 = stamp.get(17..19)?.parse().ok()?;
    // 时区缺省按 UTC+8（该上游固定用北京时间播报）。
    let offset = parse_utc_offset(body).unwrap_or(8 * 3600);
    Some(civil_to_epoch_ms(
        year, month, day, hour, minute, second, offset,
    ))
}

/// 找正文里第一个 `YYYY-MM-DD[ T]HH:MM:SS`（分隔符容忍 `-` 与 `/`）。
///
/// 逐字节扫描而不是正则：正文里出现这种形状的串只可能是时间戳，
/// 而错误文案的措辞会因为语种/版本变化 —— 认时间戳比认句子稳。
fn find_timestamp(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let digit = |slice: &[u8], index: usize| slice[index].is_ascii_digit();
    (0..=bytes.len() - 19).find(|start| {
        let slice = &bytes[*start..*start + 19];
        digit(slice, 0)
            && digit(slice, 1)
            && digit(slice, 2)
            && digit(slice, 3)
            && matches!(slice[4], b'-' | b'/')
            && digit(slice, 5)
            && digit(slice, 6)
            && matches!(slice[7], b'-' | b'/')
            && digit(slice, 8)
            && digit(slice, 9)
            && matches!(slice[10], b' ' | b'T')
            && digit(slice, 11)
            && digit(slice, 12)
            && slice[13] == b':'
            && digit(slice, 14)
            && digit(slice, 15)
            && slice[16] == b':'
            && digit(slice, 17)
            && digit(slice, 18)
    })
}

/// 解析 `UTC+8` / `UTC+08:00` / `UTC-5`。
fn parse_utc_offset(body: &str) -> Option<i64> {
    let index = body.find("UTC")? + 3;
    let rest = body.get(index..)?;
    let (sign, rest) = match rest.chars().next()? {
        '+' => (1i64, rest.get(1..)?),
        '-' => (-1i64, rest.get(1..)?),
        _ => return None,
    };
    let mut chars = rest.chars().peekable();
    let mut hours = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        hours.push(chars.next()?);
        if hours.len() == 2 {
            break;
        }
    }
    if hours.is_empty() {
        return None;
    }
    let mut minutes = String::new();
    if chars.peek() == Some(&':') {
        chars.next();
        for _ in 0..2 {
            if chars.peek().is_some_and(char::is_ascii_digit) {
                minutes.push(chars.next()?);
            }
        }
    }
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().unwrap_or(0);
    Some(sign * (hours * 3600 + minutes * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_codes_map_to_semantics() {
        assert_eq!(
            classify_openai_error(401, "unauthorized").kind,
            ErrorKind::AuthExpired
        );
        assert_eq!(
            classify_openai_error(402, "balance").kind,
            ErrorKind::QuotaExhausted
        );
        assert_eq!(
            classify_openai_error(403, "content").kind,
            ErrorKind::ContentPolicy
        );
        assert!(!classify_openai_error(403, "").retryable);
        assert_eq!(
            classify_openai_error(503, "oops").kind,
            ErrorKind::Upstream5xx
        );
        assert_eq!(
            classify_openai_error(418, "teapot").kind,
            ErrorKind::InvalidRequest
        );
    }

    #[test]
    fn upstream_diagnostics_keep_localized_tips_and_bound_large_bodies() {
        let body = format!(r#"{{"msg":"{}","requestId":"upstream-id","displayTips":{{"zh":"请提交反馈申诉"}}}}"#, "x".repeat(700));
        assert_eq!(raw(&body), body);
        let error = classify_openai_error(403, &body);
        assert!(error.describe().contains("upstream-id"));
        assert!(error.describe().contains("请提交反馈申诉"));
        let clipped = raw(&"错".repeat(10_000));
        assert!(clipped.len() < 16 * 1024 + 100);
        assert!(clipped.ends_with("已截断）"));
        assert_eq!(raw("  \n "), "");
    }

    #[test]
    fn sse_error_envelope_is_a_failure_with_upstream_diagnostics() {
        let frame = r#"{"error":{"code":"account_restricted","message":"账号已被限制使用","requestId":"stream-error-id"}}"#;
        let events = parse_openai_chunk(frame);
        assert!(matches!(&events[..], [CanonicalEvent::Error { message }] if message.contains("stream-error-id") && message.contains("账号已被限制使用")));
        let completion = crate::gateway::protocol::encode::openai::fold(&events, "test-id", "test-model", 0);
        assert!(completion.is_err(), "流内错误不能折叠成成功响应");
        assert!(completion.unwrap_err().describe().contains("account_restricted"));
        assert!(parse_openai_chunk(r#"{"error":null,"choices":[]}"#).is_empty());
    }

    #[test]
    fn reset_time_is_extracted_with_timezone() {
        let body = "429 too many requests, your usage will reset at 1970-01-02 08:00:00 UTC+8";
        assert_eq!(parse_rate_limit_reset(body), Some(86_400_000));
        let body = "... reset at 1970-01-02 00:00:00 UTC+0 ...";
        assert_eq!(parse_rate_limit_reset(body), Some(86_400_000));
    }

    #[test]
    fn missing_reset_time_means_account_level_soft_limit() {
        let error = classify_openai_error(429, "usage exceeds frequency limit");
        assert_eq!(error.kind, ErrorKind::RateLimited);
        assert_eq!(error.reset_at_ms, None);
    }

    #[test]
    fn offset_parsing_handles_half_hours_and_bare_hours() {
        assert_eq!(parse_utc_offset("UTC+8"), Some(8 * 3600));
        assert_eq!(parse_utc_offset("UTC-5"), Some(-5 * 3600));
        assert_eq!(parse_utc_offset("UTC+05:30"), Some(5 * 3600 + 1800));
        assert_eq!(parse_utc_offset("no timezone here"), None);
    }

    #[test]
    fn stream_frames_are_translated_and_empty_placeholders_dropped() {
        let text = parse_openai_chunk(
            &json!({"choices": [{"index": 0, "delta": {"content": "你"}}]}).to_string(),
        );
        assert!(matches!(&text[0], CanonicalEvent::Text { text } if text == "你"));

        // 上游的空占位：不产生事件。
        assert!(parse_openai_chunk(
            &json!({"choices": [{"index": 0, "delta": {"reasoning_content": ""}}]}).to_string()
        )
        .is_empty());
        assert!(parse_openai_chunk(
            &json!({"choices": [{"index": 0, "delta": {"tool_calls": []}}]}).to_string()
        )
        .is_empty());
        assert!(parse_openai_chunk(
            &json!({"choices": [{"index": 0, "delta": {"function_call": {"name": "", "arguments": ""}}}]})
                .to_string()
        )
        .is_empty());
    }

    #[test]
    fn tool_call_arguments_and_finish_reason_are_emitted() {
        let events = parse_openai_chunk(
            &json!({
                "choices": [{
                    "index": 0,
                    "delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "get_weather", "arguments": "{\"city\""}}]},
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
        );
        assert!(matches!(
            &events[0],
            CanonicalEvent::ToolCall { index: 0, name: Some(name), arguments_delta: Some(args), .. }
                if name == "get_weather" && args == "{\"city\""
        ));
        assert!(matches!(&events[1], CanonicalEvent::Finish { reason } if reason == "tool_calls"));
    }

    #[test]
    fn usage_aliases_are_normalised_into_one_field() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "completion_tokens_details": {"reasoning_tokens": 8},
            "prompt_cache_hit_tokens": 64,
            "credit": 0.25
        }))
        .expect("usage");
        assert_eq!(usage.cached_tokens, 64);
        assert_eq!(usage.reasoning_tokens, 8);
        assert_eq!(usage.credit, Some(0.25));

        // 优先级：prompt_tokens_details.cached_tokens 高于其他别名。
        let usage = parse_usage(&json!({
            "prompt_tokens_details": {"cached_tokens": 7},
            "prompt_cache_hit_tokens": 99
        }))
        .expect("usage");
        assert_eq!(usage.cached_tokens, 7);
    }
}
