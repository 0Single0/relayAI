//! relayAI 网关进程入口。
//!
//! 与参考项目（Tauri 内嵌）的区别：这里是一个**独立进程**，没有 `AppHandle`，
//! 配置来自环境变量，存储是 PostgreSQL。

mod gateway;
mod secrets;
mod sse;

use std::sync::Arc;

use gateway::{
    config::{Config, DEFAULT_PORT},
    server,
    state::GatewayState,
    storage::Storage,
};
use tokio::signal;

/// 内部接口共享密钥的环境变量名。
const INTERNAL_TOKEN_ENV: &str = "GATEWAY_INTERNAL_TOKEN";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 日志：不引 tracing，用 eprintln 保持依赖最小（与参考项目一致）。
    println!("[gateway] relayAI 网关启动中...");

    let master_key = secrets::load_master_key()
        .map_err(|error| anyhow::anyhow!("主密钥加载失败：{error}"))?;

    let data_source = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("缺少环境变量 DATABASE_URL"))?;

    let internal_token = std::env::var(INTERNAL_TOKEN_ENV)
        .map_err(|_| anyhow::anyhow!("缺少环境变量 {INTERNAL_TOKEN_ENV}"))?;

    let storage = Storage::open(&data_source, master_key)
        .await
        .map_err(|error| anyhow::anyhow!("存储层初始化失败：{}", error.message))?;

    // 上次进程被强杀时留下的 `running` 行永远不会有结果回写（执行状态只在内存里），
    // 不清掉它们会让界面一直显示"执行中"、账号也再也发不起新执行（SPEC 11.7）。
    match storage.fail_orphan_task_runs().await {
        Ok(0) => {}
        Ok(count) => println!("[gateway] 已清理 {count} 条中断的任务执行记录"),
        // 清理失败不该阻止启动：网关的核心职责是转发请求。
        Err(error) => eprintln!("[gateway] 清理中断的任务记录失败：{}", error.message),
    }

    let config = Config {
        port: std::env::var("GATEWAY_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_PORT),
        allow_lan: std::env::var("GATEWAY_BIND")
            .map(|value| value == "0.0.0.0")
            .unwrap_or(false),
        ..Config::default()
    };

    let state = GatewayState::initialize(config, Arc::clone(&storage))
        .await
        .map_err(|error| anyhow::anyhow!("网关状态初始化失败：{}", error.message))?;

    let channels = state.channels().len();
    let accounts = state.pool.accounts().len();
    println!("[gateway] 已加载 {channels} 个渠道、{accounts} 个账号");

    // 常驻定时任务：额度刷新与每日动作，不依赖管理台是否打开。
    gateway::ops::spawn(Arc::clone(&state));

    let handle = tokio::spawn(server::run(Arc::clone(&state), internal_token));

    // 优雅退出：收到 Ctrl-C 或 SIGTERM 时停服务，让在飞请求有时间收尾。
    signal::ctrl_c().await?;
    println!("[gateway] 收到退出信号，正在停止...");
    handle.abort();

    Ok(())
}
