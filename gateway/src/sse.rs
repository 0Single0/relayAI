//! SSE 帧重组。
//!
//! 从参考项目的 `agent/providers.rs` 抽出网关用到的部分：
//! 只需要「从任意字节块里切出完整一帧」，不需要完整的 SSE 事件解析
//! （各渠道适配器自己按帧解析，因为同一份 payload 在不同渠道含义不同）。
//!
//! 关键细节：**等分隔符到位再解 UTF-8**。中文被切在 chunk 边界上时，
//! 提前解码会得到替换字符，把正文弄坏。

/// 从缓冲区里切出一帧完整 SSE（含结尾空行），返回原始文本。
///
/// 返回 `Ok(None)` 表示还没有完整帧；缓冲区会被就地消费。
pub fn pop_sse_frame(buffer: &mut Vec<u8>) -> Result<Option<String>, String> {
    let boundary = (0..buffer.len()).find_map(|index| {
        if buffer[index..].starts_with(b"\r\n\r\n") {
            Some((index, 4))
        } else if buffer[index..].starts_with(b"\n\n") {
            Some((index, 2))
        } else {
            None
        }
    });

    let Some((index, width)) = boundary else {
        return Ok(None);
    };

    let bytes: Vec<u8> = buffer.drain(..index + width).collect();
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "模型流包含无效 UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_both_delimiters() {
        let mut buffer = b"data: one\n\ndata: two\r\n\r\n".to_vec();
        assert_eq!(pop_sse_frame(&mut buffer).unwrap().as_deref(), Some("data: one\n\n"));
        assert_eq!(
            pop_sse_frame(&mut buffer).unwrap().as_deref(),
            Some("data: two\r\n\r\n")
        );
        assert!(pop_sse_frame(&mut buffer).unwrap().is_none());
    }

    #[test]
    fn waits_for_split_utf8() {
        let frame = "data: {\"text\":\"你好\"}\n\n".as_bytes();
        let split = frame.iter().position(|b| *b >= 0x80).unwrap() + 1;

        let mut buffer = frame[..split].to_vec();
        assert!(
            pop_sse_frame(&mut buffer).unwrap().is_none(),
            "半帧时不能解码，否则中文会被替换成乱码"
        );

        buffer.extend_from_slice(&frame[split..]);
        let parsed = pop_sse_frame(&mut buffer).unwrap().unwrap();
        assert!(parsed.contains("你好"));
        assert!(buffer.is_empty());
    }
}
