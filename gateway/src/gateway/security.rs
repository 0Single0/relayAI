//! 设备指纹派生与凭据信封。
//!
//! 指纹：同账号恒定、跨账号天然隔离，避免多账号被上游关联风控。
//! 参考实现的 `derive_id(uid, salt)` 只按 uid 派生；这里把 `channel_id` 也拌进去，
//! 同一个 uid 在不同渠道拿到不同指纹。
//!
//! 信封：主密钥来自环境变量（见 [`crate::secrets`]），密文进 PG。
//! 上游每次刷新都会轮换 token，逐条进系统凭据库既慢又污染凭据库；
//! 换成"一把主密钥 + 库内密文"更合适。

use sha2::{Digest, Sha256};

/// 凭据信封：AES-256-GCM，`base64(nonce || ciphertext)`。
pub mod envelope {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use base64::Engine as _;

    const NONCE_LEN: usize = 12;

    /// 用给定主密钥加密。
    pub fn seal(key: &[u8; 32], plaintext: &str) -> Result<String, String> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "密钥长度不对".to_string())?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce_bytes).map_err(|error| format!("取随机数失败：{error}"))?;
        let nonce = Nonce::try_from(&nonce_bytes[..]).map_err(|_| "nonce 长度不对".to_string())?;
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| "加密失败".to_string())?;
        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        Ok(base64::engine::general_purpose::STANDARD.encode(combined))
    }

    /// 用给定主密钥解密。密文被改动或密钥不匹配都返回错误，不静默返回垃圾。
    pub fn open(key: &[u8; 32], sealed: &str) -> Result<String, String> {
        let combined = base64::engine::general_purpose::STANDARD
            .decode(sealed.as_bytes())
            .map_err(|_| "密文不是合法 base64".to_string())?;
        if combined.len() <= NONCE_LEN {
            return Err("密文长度不对".to_string());
        }
        let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).map_err(|_| "nonce 长度不对".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "密钥长度不对".to_string())?;
        let plaintext = cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| "解密失败（密钥不匹配或密文被改动）".to_string())?;
        String::from_utf8(plaintext).map_err(|_| "明文不是 UTF-8".to_string())
    }
}

/// 稳定派生一个 36 位十六进制标识（长度对齐参考实现，便于直接替换）。
pub fn derive_id(channel_id: &str, uid: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{salt}:{channel_id}:{uid}").as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex.truncate(36);
    hex
}

/// 稳定短哈希：用于 API Key 作用域、日志指纹这类"要能比较但不必还原"的场景。
pub fn short_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(32);
    for byte in digest.iter().take(8) {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// 每请求唯一的 `X-Request-ID`：稳定前缀 + 单调序号，跨请求可追溯又不暴露账号。
pub fn request_id(channel_id: &str, uid: &str, sequence: u64) -> String {
    format!(
        "{}-{:06}",
        derive_id(channel_id, uid, "req"),
        sequence % 1_000_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_per_account_and_channel() {
        let a = derive_id("wb-cn", "uid-1", "machine");
        assert_eq!(a, derive_id("wb-cn", "uid-1", "machine"), "同账号必须恒定");
        assert_eq!(a.len(), 36);
        assert_ne!(a, derive_id("wb-cn", "uid-2", "machine"), "换账号必须不同");
        assert_ne!(a, derive_id("wb-intl", "uid-1", "machine"), "换渠道必须不同");
    }

    #[test]
    fn envelope_round_trips_and_detects_tampering() {
        let key = [7u8; 32];
        let sealed = envelope::seal(&key, "access-token-1").expect("加密");
        assert_ne!(sealed, "access-token-1");
        assert_eq!(envelope::open(&key, &sealed).expect("解密"), "access-token-1");
        // 换密钥解不开。
        assert!(envelope::open(&[8u8; 32], &sealed).is_err());
        // 密文被改动要报错。
        let mut tampered = sealed.clone();
        tampered.push('X');
        assert!(envelope::open(&key, &tampered).is_err());
        assert!(envelope::open(&key, "not-base64!!").is_err());
    }
}
