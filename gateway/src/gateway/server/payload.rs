//! 请求内容留存（默认关闭）。
//!
//! 用途：`usage_logs` 只有元数据，出问题时看不到"用户到底发了什么"。
//! 打开后可以逐条回放，复现"模型没按格式回""prompt 太长被截断"这类问题。
//!
//! **隐私边界**（这三条是打开这个功能的前提）：
//!   1. 默认关闭，必须显式打开；
//!   2. 只存**语义内容**（messages / system / tools / 最终文本），
//!      不存原始 HTTP 体 —— 那里面有客户端自己的 header，可能含认证信息；
//!   3. 保留天数独立于用量日志，通常更短。
//!
//! 大小上限：单条超过 [`MAX_BYTES`] 就截断并标记，避免一个超长 prompt
//! 把库撑爆（用户贴一整本书进对话是真实会发生的）。

use crate::gateway::protocol::cir::CanonicalRequest;
use crate::gateway::state::SharedGateway;

/// 单条内容留存的上限（字节）。
///
/// 256 KiB 约等于 6 万汉字，足以覆盖绝大多数真实对话；
/// 再大的基本都是"把整个文件贴进来"，那种场景看开头也够定位问题。
const MAX_BYTES: usize = 256 * 1024;

/// 把请求序列化成脱敏 JSON；开关关闭时返回 `None`（不产生任何开销）。
pub async fn request_json(
    state: &SharedGateway,
    request: &CanonicalRequest,
) -> Option<String> {
    if !state.storage.payload_enabled().await {
        return None;
    }

    let value = serde_json::json!({
        "model": request.model,
        "system": request.system,
        "messages": request.messages.iter().map(|message| {
            serde_json::json!({
                "role": message.role,
                "content": message.content.iter().filter_map(|block| block.as_text()).collect::<Vec<_>>(),
                "tool_calls": message.tool_calls.len(),
            })
        }).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
        "stream": request.stream,
    });

    Some(truncate(value.to_string()))
}

/// 超长就截断，并在末尾留一个显式标记。
///
/// 标记是必要的：截断后的 JSON 不是合法 JSON，但读者（人）需要知道
/// "这不是全部内容"，否则会以为请求本身就这么短。
fn truncate(text: String) -> String {
    if text.len() <= MAX_BYTES {
        return text;
    }

    // 按字符边界切，避免把多字节 UTF-8 字符劈开导致存储层报编码错误。
    let mut end = MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    format!("{}…[已截断，原长 {} 字节]", &text[..end], text.len())
}
