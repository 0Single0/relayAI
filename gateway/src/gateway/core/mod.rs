//! 网关核心：账号池、调度、记账。
//!
//! P0 只落地调度骨架（候选 → 轮询挑选 → 重试排除 → 冷却策略 → 并发槽）；
//! 矩阵、亲和、打分分层在 P1 接进来，接入点见 `dispatch::open_chat_session` 的注释。

pub mod affinity;
pub mod billing;
pub mod dispatch;
pub mod matrix;
pub mod pool;
pub mod refresh;
pub mod router;
pub mod usage;
