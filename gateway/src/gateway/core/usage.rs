//! 用量估算：上游没给 usage 时的兜底（CJK 感知），并标记 `estimated`。
//!
//! 只用于统计与守卫，不用于计费口径；能拿到上游 usage 就一律以它为准。

use crate::gateway::protocol::cir::{Block, CanonicalRequest, Usage};

/// 请求的输入 token 估算（CJK 感知），只在响应没有 usage 时使用。
pub fn estimate_request_tokens(request: &CanonicalRequest) -> u64 {
    let system: u64 = request
        .system
        .iter()
        .filter_map(Block::as_text)
        .map(estimate_tokens)
        .sum();
    let messages: u64 = request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(Block::as_text)
        .map(estimate_tokens)
        .sum();
    system + messages
}

/// 粗略 token 估算：CJK 一字一 token，其他按 4 字符一 token。
pub fn estimate_tokens(text: &str) -> u64 {
    let mut cjk = 0u64;
    let mut other = 0u64;
    for character in text.chars() {
        if is_cjk(character) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3000..=0x303F |   // CJK 标点
        0x3040..=0x30FF |   // 假名
        0x3400..=0x4DBF |   // 扩展 A
        0x4E00..=0x9FFF |   // 基本区
        0xF900..=0xFAFF |   // 兼容表意
        0xFF00..=0xFFEF     // 全角
    )
}

/// 上游没给 usage 时的估算值：输入用请求估算值，输出按已产出文本长度。
pub fn estimate_completion(prompt_tokens: u64, produced: &str) -> Usage {
    let completion_tokens = estimate_tokens(produced);
    Usage {
        prompt_tokens,
        completion_tokens,
        reasoning_tokens: 0,
        cached_tokens: 0,
        total_tokens: prompt_tokens + completion_tokens,
        credit: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::cir::{Message, Role};

    #[test]
    fn cjk_counts_per_character_and_latin_by_quarters() {
        assert_eq!(estimate_tokens("你好世界"), 4);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens(""), 0);
        assert!(estimate_tokens("中文abc") >= 3);
    }

    #[test]
    fn estimation_fills_both_directions() {
        let usage = estimate_completion(2, "abcdefgh");
        assert_eq!(usage.prompt_tokens, 2);
        assert_eq!(usage.completion_tokens, 2);
        assert_eq!(usage.total_tokens, 4);
    }

    #[test]
    fn request_estimation_covers_system_and_messages() {
        let request = CanonicalRequest {
            model: "glm-5.3".into(),
            system: vec![Block::text("系统提示")],
            messages: vec![Message {
                role: Role::User,
                content: vec![Block::text("你好")],
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
            stream: false,
            reasoning: None,
            metadata: Default::default(),
            raw: serde_json::json!({}),
        };
        assert_eq!(estimate_request_tokens(&request), 6);
    }
}
