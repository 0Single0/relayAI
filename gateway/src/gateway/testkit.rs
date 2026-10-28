//! 测试辅助：从环境变量读数据库，加载真实账号。
//!
//! 参考项目的探针测试从桌面端的 APPDATA 目录读 SQLite + 系统凭据库；
//! 服务端版本没有这两样东西，所以统一改成读 `DATABASE_URL` 与
//! `RELAYAI_MASTER_KEY` —— 与生产完全同一条路径。
//!
//! 为什么集中在这里：探针散落在五个渠道文件里，各自抄一份连接逻辑
//! 迟早会分叉（参考项目就有 5 份几乎一样的 `Fixture::load`）。

#![cfg(test)]

use std::sync::Arc;

use crate::gateway::{channels::AccountSnapshot, storage::Storage};

/// 打开数据库并返回存储层。
///
/// 调用方应放在 `#[ignore]` 的测试里：没有环境变量时会 panic，
/// 而普通 `cargo test` 不该要求开发者先配好数据库。
pub async fn open_storage() -> Arc<Storage> {
    let data_source = std::env::var("DATABASE_URL")
        .expect("探针测试需要 DATABASE_URL（例如 postgresql://postgres:***@localhost:5432/relayai）");
    let master_key = crate::secrets::load_master_key()
        .expect("探针测试需要 RELAYAI_MASTER_KEY");

    Storage::open(&data_source, master_key)
        .await
        .expect("打开数据库失败")
}

/// 取指定渠道的第一个账号快照。
pub async fn account_snapshot(storage: &Storage, channel_id: &str) -> AccountSnapshot {
    let records = storage.list_accounts().await.expect("读账号失败");

    let record = records
        .into_iter()
        .find(|record| record.channel_id == channel_id)
        .unwrap_or_else(|| panic!("库里没有 {channel_id} 账号"));

    let credential = record
        .credential
        .unwrap_or_else(|| panic!("{channel_id} 账号没有凭据"));

    AccountSnapshot {
        channel_id: record.channel_id.clone(),
        uid: record.uid.clone(),
        access_token: Some(credential.access_token.clone()),
        refresh_token: credential.refresh_token.clone(),
        enterprise_id: credential.enterprise_id.clone(),
        access_key_id: credential.access_key_id.clone(),
        secret_key: credential.secret_key.clone(),
        code_verifier: credential.code_verifier.clone(),
        base_url: (!record.base_url.is_empty()).then(|| record.base_url.clone()),
        protocol: (!record.protocol.is_empty()).then(|| record.protocol.clone()),
        platform: (!record.platform.is_empty()).then(|| record.platform.clone()),
        options: (!record.options.is_null()).then(|| record.options.clone()),
    }
}
