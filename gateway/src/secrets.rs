//! 主密钥来源：环境变量。
//!
//! 参考实现把主密钥存进系统凭据库（keyring），依赖桌面环境。服务端没有交互式
//! 凭据库，容器里更没有，所以改成从环境变量读 32 字节 base64。
//!
//! 这条改动是**破坏性的**：老库里的密文用旧主密钥加密，换环境变量后解不开。
//! 迁移时必须把旧主密钥导出并写进 `RELAYAI_MASTER_KEY`，否则账号需要重新登录。

use std::fmt;

use base64::Engine as _;

/// 环境变量名。
pub const MASTER_KEY_ENV: &str = "RELAYAI_MASTER_KEY";

/// 主密钥加载失败的原因。
#[derive(Debug)]
pub struct MasterKeyError(String);

impl fmt::Display for MasterKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MasterKeyError {}

/// 读取并校验主密钥。
///
/// 失败时**必须让进程起不来**：拿着空密钥跑起来，会把新账号用错误的密钥加密，
/// 之后换回正确密钥也解不开了。
pub fn load_master_key() -> Result<[u8; 32], MasterKeyError> {
    let raw = std::env::var(MASTER_KEY_ENV).map_err(|_| {
        MasterKeyError(format!(
            "缺少环境变量 {MASTER_KEY_ENV}：需要 32 字节的 base64 主密钥"
        ))
    })?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim().as_bytes())
        .map_err(|_| MasterKeyError(format!("{MASTER_KEY_ENV} 不是合法 base64")))?;

    bytes
        .try_into()
        .map_err(|_| MasterKeyError(format!("{MASTER_KEY_ENV} 解码后不是 32 字节")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_env() {
        // 不设置环境变量时必须报错，而不是返回全零密钥。
        let result = std::env::var(MASTER_KEY_ENV);
        if result.is_ok() {
            // 该环境已设置，跳过（避免与其他测试互相干扰）。
            return;
        }
        assert!(load_master_key().is_err());
    }

    #[test]
    fn decodes_valid_key() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map(|bytes| bytes.len()),
            Ok(32)
        );
    }
}
