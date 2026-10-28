//! relayAI 网关：进程内 HTTP 服务 + 多渠道账号池调度。
//!
//! 来源：从参考项目（dynamic-island）的 `src-tauri/src/gateway/` 迁移。
//! 相对原实现的变化：
//!   · 去掉 Tauri 运行时（`AppHandle` / `tauri::async_runtime`），改独立进程；
//!   · 主密钥从系统凭据库改为环境变量（见 [`crate::secrets`]）；
//!   · 存储从 SQLite 改为 PostgreSQL；
//!   · 面板命令层（原 `commands/`）改为 `/internal/*` HTTP 路由。
//!
//! 依赖方向：`server → core → channels → transport`，`channels` 不得引用 `core`。

pub mod channels;
pub mod config;
pub mod core;
pub mod errors;
pub mod keyauth;
pub mod notify;
pub mod ops;
pub mod protocol;
pub mod security;
pub mod server;
pub mod state;
pub mod storage;
pub mod testkit;
pub mod timeutil;
pub mod transport;
