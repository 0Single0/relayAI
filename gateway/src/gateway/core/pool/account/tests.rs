use super::*;

#[test]
fn temporary_breaker_keeps_affinity_but_other_blocks_still_allow_migration() {
    let mut account = PoolAccount::new("workbuddy", "account-a");
    account.requires_credential = false;
    let now = crate::gateway::timeutil::now_ms();
    let policy = CooldownConfig::default();
    assert!(!account.affinity_recovering("m", now));
    for _ in 0..policy.breaker_after {
        account.apply_error(&GatewayError::network("temporary failure"), &policy, now);
    }
    assert!(!account.is_ready("m", now));
    assert!(account.affinity_recovering("m", now));
    account.apply_error(
        &GatewayError::new(
            ErrorKind::RateLimited,
            ErrorScope::AccountModel("m".into()),
            429,
            "model limit",
            true,
        ),
        &policy,
        now,
    );
    assert!(!account.affinity_recovering("m", now));
    assert!(account.affinity_recovering("another-model", now));
    account.on_success("m");
    assert!(account.is_ready("m", now));
    assert!(!account.affinity_recovering("m", now));
}

#[test]
fn breaker_does_not_preserve_disabled_missing_credential_or_quota_exhausted_accounts() {
    let policy = CooldownConfig {
        breaker_after: 1,
        ..Default::default()
    };
    let now = crate::gateway::timeutil::now_ms();
    let mut account = PoolAccount::new("workbuddy", "account-a");
    account.requires_credential = false;
    account.apply_error(&GatewayError::network("temporary failure"), &policy, now);
    assert!(account.affinity_recovering("m", now));
    account.enabled = false;
    assert!(!account.affinity_recovering("m", now));
    account.enabled = true;
    account.membership_enabled = false;
    assert!(!account.affinity_recovering("m", now));
    account.membership_enabled = true;
    account.requires_credential = true;
    assert!(!account.affinity_recovering("m", now));
    account.requires_credential = false;
    account.apply_error(
        &GatewayError::new(
            ErrorKind::QuotaExhausted,
            ErrorScope::Account,
            402,
            "quota exhausted",
            true,
        ),
        &policy,
        now,
    );
    assert!(!account.affinity_recovering("m", now));
}
use crate::gateway::{channels::Credential, timeutil::now_ms};

fn account() -> Arc<PoolAccount> {
    let mut account = PoolAccount::new("fake", "u1");
    account.nickname = "测试账号".into();
    account.requires_credential = false;
    account.set_max_inflight(1);
    Arc::new(account)
}

/// CodeArts 的长期 AK/SK **没有 access_token**，但它明明是有效凭据。
///
/// 早先 `is_ready` 只看 `access_token.is_empty()`，于是导入 `credentials.csv`
/// 之后账号立刻被判成"没有凭据 / 需要先登录"——这就是"刚导入就受限"的成因。
#[test]
fn ak_sk_only_credentials_count_as_present() {
    let mut account = PoolAccount::new("codearts", "u1");
    account.requires_credential = true;
    account.set_credentials(Credential {
        // 长期 AK/SK：没有 token。
        access_token: String::new(),
        access_key_id: Some("AK1".into()),
        secret_key: Some("SK1".into()),
        ..Credential::default()
    });
    assert!(account.credentials().is_present(), "AK/SK 是有效的凭据形态");
    assert!(account.is_ready("snap-chat", now_ms()), "不该被判成没登录");
    assert!(
        account.blocked_reasons("snap-chat", now_ms()).is_empty(),
        "不该报「需要先登录」"
    );
}

/// 真正的空凭据仍然要判成不可用（别把上一条的修复做成"永远就绪"）。
#[test]
fn a_completely_empty_credential_is_still_not_ready() {
    let mut account = PoolAccount::new("codearts", "u2");
    account.requires_credential = true;
    assert!(!account.credentials().is_present());
    assert!(!account.is_ready("snap-chat", now_ms()));
    assert!(!account.blocked_reasons("snap-chat", now_ms()).is_empty());
}
#[test]
fn missing_credential_blocks_only_channels_that_need_one() {
    let mut needs = PoolAccount::new("wb-cn", "u1");
    needs.enabled = true;
    assert!(!needs.is_ready("glm-5.3", now_ms()), "真实渠道没凭据不可用");
    needs.set_credentials(Credential {
        access_token: "token".into(),
        ..Credential::default()
    });
    assert!(needs.is_ready("glm-5.3", now_ms()));

    let open = {
        let mut account = PoolAccount::new("fake", "u1");
        account.requires_credential = false;
        account
    };
    assert!(open.is_ready("fake-model", now_ms()), "假渠道不需要凭据");
}

#[test]
fn static_credential_without_expiry_stays_ready() {
    // 自定义供应商的密钥没有过期时间。若它参与"临期刷新"判断，
    // 每次请求都会尝试刷新并失败 → 账号被当成不可用（等于接进来也用不了）。
    let mut account = PoolAccount::new("my-provider", "default");
    account.static_credential = true;
    account.set_credentials(Credential {
        access_token: "sk-1".into(),
        expires_at_ms: None,
        ..Credential::default()
    });
    assert!(account.is_ready("gpt-x", now_ms()));
    // 保险丝本身仍在：它标记的是"永不过期"，不是"绕过可用性判断"。
    assert!(
        account.credentials().is_expiring(now_ms(), 120_000),
        "无过期时间的凭据确实会被判临期 —— 所以必须靠 static 标记跳过刷新"
    );
}

#[test]
fn auth_failure_cools_account_for_the_configured_window() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    account.apply_error(
        &GatewayError::new(ErrorKind::AuthExpired, ErrorScope::Account, 401, "x", true),
        &policy,
        now,
    );
    assert!(!account.is_ready("m", now));
    assert!(account.is_ready("m", now + (policy.auth_secs + 1) * 1000));
}

/// 模型限额是本渠道的常态（workbuddy 的 6004）：一个模型被限，账号必须照常服务其他模型。
#[test]
fn model_scoped_limit_leaves_sibling_models_serviceable() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    account.apply_error(
        &GatewayError::new(
            ErrorKind::RateLimited,
            ErrorScope::AccountModel("deepseek-v4.1-flash".into()),
            429,
            "该模型被上游限流",
            true,
        ),
        &policy,
        now,
    );
    assert!(
        !account.is_ready("deepseek-v4.1-flash", now),
        "被限的模型不可用"
    );
    assert!(account.is_ready("glm-5.3", now), "同账号其他模型照常可用");
    assert_eq!(account.cooldown_until_ms(), 0, "不该把整个账号拉黑");

    /*
      面板仍要能看到"哪个模型被限"，但**换了一个出口**。

      早先它塞在 `blocked_reasons` 里 —— 那个列表的语义是
      "账号整体不可用"，后端据此把 status 算成 `cooldown`，
      于是整号显示成冷却中。现在改由 `model_cooldowns_snapshot()` 提供，
      界面用单独一列展示。
    */
    assert!(
        account.blocked_reasons("", now).is_empty(),
        "模型级限流不该进账号级阻断原因"
    );
    let limits = account.model_cooldowns_snapshot(now);
    assert!(
        limits
            .iter()
            .any(|(model, _)| model == "deepseek-v4.1-flash"),
        "面板要能从快照里看到是哪个模型被限：{limits:?}"
    );
}

/// 拿不到恢复时刻也要停在**模型**上：不能因为解析不出时间就停整个账号。
#[test]
fn model_scoped_limit_without_reset_still_only_cools_that_model() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    account.apply_error(
        &GatewayError::new(
            ErrorKind::RateLimited,
            ErrorScope::AccountModel("glm-5.3".into()),
            429,
            "该模型被上游限流",
            true,
        ),
        &policy,
        now,
    );
    assert!(!account.is_ready("glm-5.3", now));
    assert!(account.is_ready("deepseek-v4.1-flash", now));
    assert_eq!(account.cooldown_until_ms(), 0);
    // 退避窗口有效：到点自己恢复。
    assert!(account.is_ready("glm-5.3", now + (policy.soft_base_secs + 1) * 1000));
}

/// 上游说了恢复时刻但没说是哪个模型 → 按那个时刻停整个账号（旧行为是静默 no-op）。
#[test]
fn account_level_limit_with_reset_cools_until_that_moment() {
    let account = account();
    let now = now_ms();
    account.apply_error(
        &GatewayError::new(ErrorKind::RateLimited, ErrorScope::Account, 429, "x", true)
            .with_reset_at(Some(now + 120_000)),
        &CooldownConfig::default(),
        now,
    );
    assert!(!account.is_ready("glm-5.3", now));
    assert_eq!(
        account.cooldown_until_ms(),
        now + 120_000,
        "停到上游说的时刻"
    );
}

#[test]
fn rate_limit_with_reset_only_cools_that_model() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    let error = GatewayError::new(
        ErrorKind::RateLimited,
        ErrorScope::AccountModel("glm-5.3".into()),
        429,
        "x",
        true,
    )
    .with_reset_at(Some(now + 60_000));
    account.apply_error(&error, &policy, now);
    assert!(!account.is_ready("glm-5.3", now));
    assert!(account.is_ready("hy3", now), "其他模型不该被连坐");
    assert!(account.is_ready("glm-5.3", now + 61_000));
}

#[test]
fn soft_rate_limit_backs_off_exponentially_and_then_clears() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    let error = || GatewayError::new(ErrorKind::RateLimited, ErrorScope::Account, 429, "x", true);
    account.apply_error(&error(), &policy, now);
    let first = account.cooldown_until_ms() - now;
    account.runtime.cooldown_until.store(0, Ordering::SeqCst);
    account.runtime.soft_streak.store(1, Ordering::SeqCst);
    account.apply_error(&error(), &policy, now);
    let second = account.cooldown_until_ms() - now;
    assert_eq!(first, policy.soft_base_secs * 1000);
    assert_eq!(second, policy.soft_base_secs * 2000);

    account.on_success("m");
    assert_eq!(account.cooldown_until_ms(), 0);
    assert!(account.is_ready("m", now));
}

#[test]
fn quota_exhaustion_parks_until_next_local_4am() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    account.apply_error(
        &GatewayError::new(
            ErrorKind::QuotaExhausted,
            ErrorScope::Account,
            402,
            "x",
            true,
        ),
        &policy,
        now,
    );
    let until = account.cooldown_until_ms();
    assert!(until > now);
    assert!(until <= now + 25 * 3600 * 1000);
}

#[test]
fn repeated_5xx_trips_the_breaker_only_after_the_threshold() {
    let account = account();
    let policy = CooldownConfig::default();
    let now = now_ms();
    let error = || GatewayError::upstream(503, "boom");
    for _ in 0..policy.breaker_after - 1 {
        account.apply_error(&error(), &policy, now);
        assert_eq!(account.runtime.breaker_until.load(Ordering::SeqCst), 0);
    }
    account.apply_error(&error(), &policy, now);
    assert!(account.runtime.breaker_until.load(Ordering::SeqCst) > now);
}

#[test]
fn inflight_slot_is_released_by_the_guard() {
    let account = account();
    let guard = account.try_acquire().expect("有空位");
    assert!(account.try_acquire().is_none(), "上限 1 时第二次必须失败");
    drop(guard);
    assert!(account.try_acquire().is_some(), "释放后应当能再取");
}

#[test]
fn backlog_is_capped() {
    assert_eq!(backoff_secs(600, 0, 7200), 600);
    assert_eq!(backoff_secs(600, 3, 7200), 4800);
    assert_eq!(backoff_secs(600, 10, 7200), 7200);
}

/// 模型级限流**不能**把账号标成"不可用"。
///
/// 这是曾经的 bug：`blocked_reasons` 把模型级也塞进去，
/// 后端看到列表非空就把 status 算成 `cooldown` ——
/// 界面显示"冷却中"，用户以为号废了去删号重建。
/// 而真相是号照常服务其他模型。
#[test]
fn model_limit_does_not_block_account() {
    let mut item = PoolAccount::new("wb", "u1");
    item.requires_credential = false;
    let handle = Arc::new(item);
    let now = now_ms();

    // 模拟 6004：模型级限流。
    let error = GatewayError::new(
        ErrorKind::RateLimited,
        ErrorScope::AccountModel("glm-5.3".into()),
        429,
        "该模型被上游限流",
        true,
    )
    .with_reset_at(Some(now + 600_000));
    handle.apply_error(&error, &CooldownConfig::default(), now);

    // 账号整体仍然可用。
    assert!(
        handle.is_ready("glm-5.3", now) == false,
        "被限流的那个模型当然不可用"
    );
    assert!(
        handle.is_ready("deepseek-v4.1-flash", now),
        "同账号的其他模型必须照常可用"
    );
    assert!(
        handle.blocked_reasons("", now).is_empty(),
        "模型级限流不该出现在账号级阻断原因里"
    );

    // 但明细要能查到，界面靠它渲染「模型」列。
    let limits = handle.model_cooldowns_snapshot(now);
    assert!(
        limits.iter().any(|(model, _)| model == "glm-5.3"),
        "模型级限流应能在快照里查到：{limits:?}"
    );
}

/// 重置要能一次清掉账号级与模型级。
#[test]
fn reset_clears_both_levels() {
    let mut item = PoolAccount::new("wb", "u2");
    item.requires_credential = false;
    let handle = Arc::new(item);
    let now = now_ms();

    // 账号级冷却。
    let auth = GatewayError::new(
        ErrorKind::AuthExpired,
        ErrorScope::Account,
        401,
        "凭据过期",
        false,
    );
    handle.apply_error(&auth, &CooldownConfig::default(), now);
    // 模型级限流。
    let model = GatewayError::new(
        ErrorKind::RateLimited,
        ErrorScope::AccountModel("glm-5.3".into()),
        429,
        "该模型被上游限流",
        true,
    )
    .with_reset_at(Some(now + 600_000));
    handle.apply_error(&model, &CooldownConfig::default(), now);

    assert!(handle.reset_cooldowns(), "应当报告清掉了东西");
    assert!(handle.is_ready("glm-5.3", now), "重置后所有模型都应可用");
    assert!(handle.model_cooldowns_snapshot(now).is_empty());
    assert!(!handle.reset_cooldowns(), "第二次应报告没有可清的");
}
