//! 凭据保鲜：选号前检查，临期则**按账号串行**刷新并落库。
//!
//! 为什么必须串行：上游每次刷新都会轮换 refresh_token，
//! 并发刷新会让后一次拿着已作废的旧 token 去换，结果是账号被刷废。

use crate::gateway::{
    channels::Credential, core::pool::PoolAccount, errors::GatewayError, state::GatewayState,
    timeutil::now_ms,
};

/// 安全边距：剩余寿命不足 2 分钟就先刷新（与参考实现一致）。
pub const SAFETY_MARGIN_MS: u64 = 120_000;

/// 确保账号凭据可用。返回 false = 该账号当前不可用（没凭据或刷新失败）。
pub async fn ensure_credential(state: &GatewayState, account: &PoolAccount) -> bool {
    let credential = account.credentials();
    // `is_present()` 而不是只看 token：CodeArts 的长期 AK/SK 没有 token。
    if !credential.is_present() {
        return !account.requires_credential;
    }
    // 静态密钥（自定义供应商）没有刷新端点，也永远不会过期：
    // 走到刷新只会拿到 "不支持刷新" 并把账号判为不可用。
    if account.static_credential {
        return true;
    }
    if !credential.is_expiring(now_ms(), SAFETY_MARGIN_MS) {
        return true;
    }
    // 选号路径：只在临期时刷新。
    match refresh(state, account, false).await {
        Ok(_) => true,
        Err(error) => {
            eprintln!(
                "[gateway] 账号 {} 刷新失败（本轮换号）：{}",
                account.key, error.message
            );
            false
        }
    }
}

/// 刷新凭据。
///
/// `force = false`：只在**确实临期**时才刷新 —— 选号路径用它，避免每请求都打上游。
/// `force = true`：**无条件**刷新 —— 面板那个"刷新凭据"按钮必须走这条。
///
/// 为什么需要 force：上游可能因为风控 / 改密 / 异地登录**提前吊销** token，
/// 而本地 `expires_at_ms` 还没到。此时"未到期就跳过"会让刷新变成空操作：
/// 用户点了按钮、界面显示成功，拿回来的仍是那个已被吊销的 token。
/// Trae 的 `code=1001`（not able to authenticate you）正是这种情形。
pub async fn refresh(
    state: &GatewayState,
    account: &PoolAccount,
    force: bool,
) -> Result<Credential, GatewayError> {
    let _serial = account.runtime.refresh_lock.lock().await;

    // 等锁期间别的请求可能已经刷好了：加锁后再确认一次，避免重复刷新把 token 轮换两次。
    // force 时**不做这个短路** —— 用户明确要求换一个新的。
    let current = account.credentials();
    if !force && !current.access_token.is_empty() && !current.is_expiring(now_ms(), 0) {
        return Ok(current);
    }

    let adapter = state.adapter_for(account).ok_or_else(|| {
        GatewayError::internal(format!(
            "账号 {}/{} 没有平台配置，无法刷新凭据",
            account.key.channel_id, account.key.uid
        ))
    })?;
    let refreshed = adapter
        .refresh(&state.client, &account.snapshot())
        .await?
        .ok_or_else(|| GatewayError::unsupported("该渠道不支持刷新凭据，请重新登录"))?;

    account.set_credentials(refreshed.clone());
    // 内存里已生效；落库失败只影响重启后的可用性，不该让本次请求失败。
    if let Err(error) = state
        .storage
        .set_credential(&account.key.channel_id, &account.key.uid, Some(&refreshed))
        .await
    {
        eprintln!("[gateway] 新凭据落库失败：{}", error.message);
    }
    eprintln!(
        "[gateway] 账号 {} 凭据已刷新，{}",
        account.key,
        expiry_hint(&refreshed)
    );
    Ok(refreshed)
}

fn expiry_hint(credential: &Credential) -> String {
    match credential.expires_at_ms {
        Some(expires_at) => {
            let remaining = expires_at.saturating_sub(now_ms()) / 60_000;
            format!("剩余 {remaining} 分钟")
        }
        None => "上游未给过期时间".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channels::Credential;

    /// `is_expiring` 是"要不要**主动**刷新"的依据，而不是"是否仍然有效"。
    ///
    /// 上游可以提前吊销 token（风控 / 改密 / 异地登录），此时本地过期时间还没到 ——
    /// 这正是 Trae `code=1001` 的成因：凭据看起来"还剩 14 天"，实际已经不认了。
    /// 所以面板的强制刷新绝不能因为"未到期"就短路。
    #[test]
    fn a_credential_that_is_not_expiring_may_still_be_revoked() {
        let far_future = now_ms() + 14 * 24 * 3600 * 1000;
        let credential = Credential {
            access_token: "STILL-VALID-LOCALLY".into(),
            expires_at_ms: Some(far_future),
            ..Default::default()
        };
        assert!(
            !credential.is_expiring(now_ms(), SAFETY_MARGIN_MS),
            "本地看着还能用 —— 但上游可能已经吊销了"
        );
        // 强制刷新（force=true）不会走"未到期即返回"的短路；
        // 非强制路径（选号）才用它省一次上游调用。
    }

    /// 没有过期时间的凭据一律按"需要刷新"处理（宁可多刷，不能拿死 token 打上游）。
    #[test]
    fn a_credential_without_expiry_always_needs_refreshing() {
        let credential = Credential {
            access_token: "T".into(),
            expires_at_ms: None,
            ..Default::default()
        };
        assert!(credential.is_expiring(now_ms(), 0));
    }
}
