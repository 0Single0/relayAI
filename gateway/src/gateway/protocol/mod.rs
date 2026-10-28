//! 协议层：三套下游协议（Chat Completions / Anthropic Messages / Responses）
//! 与渠道报文之间的**唯一**中间表示（CIR）。
//!
//! 没有 CIR，3 个入口 × N 个渠道就是 3N 份转换代码；有了它是 3 + N 份。
//! 入口适配器只做「协议 ↔ CIR」，渠道适配器只做「CIR ↔ 上游报文 + 上游 SSE → 事件」。

pub mod cir;
pub mod decode;
pub mod encode;
