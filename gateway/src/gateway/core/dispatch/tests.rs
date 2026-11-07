use super::*;

fn test_binding(channel_id: &str, uid: &str) -> Binding {
    Binding {
        channel_id: channel_id.into(),
        uid: uid.into(),
        upstream_model: "glm-5.3".into(),
        expires_at_ms: 123,
        hits: 4,
    }
}

#[test]
fn overflow_account_error_does_not_break_original_affinity() {
    let binding = test_binding("channel-a", "account-a");
    let overflow = AccountKey::new("channel-b", "account-b");
    let error = GatewayError::network("temporary upstream failure");

    assert!(should_preserve_affinity_after_error(
        Some(&binding),
        &overflow,
        &error
    ));
}

#[test]
fn temporary_network_failure_on_bound_account_keeps_affinity() {
    let binding = test_binding("channel-a", "account-a");
    let bound = AccountKey::new("channel-a", "account-a");
    let error = GatewayError::network("temporary upstream failure");

    assert!(should_preserve_affinity_after_error(
        Some(&binding),
        &bound,
        &error
    ));
}

#[test]
fn upstream_5xx_keeps_original_binding_after_successful_fallback() {
    let binding = test_binding("channel-a", "account-a");
    let bound = AccountKey::new("channel-a", "account-a");
    for status in [500, 502, 503, 504] {
        let keep = should_preserve_affinity_after_error(
            Some(&binding),
            &bound,
            &GatewayError::upstream(status, "temporary failure"),
        );
        assert!(keep);
        let overflow = AccountKey::new("channel-a", "account-b");
        assert_eq!(
            binding_after_success(keep, Some(&binding), &overflow, "glm-5.3"),
            binding
        );
    }
}

#[test]
fn definite_account_or_model_failures_allow_binding_migration() {
    let binding = test_binding("channel-a", "account-a");
    let bound = AccountKey::new("channel-a", "account-a");
    for kind in [
        ErrorKind::AuthExpired,
        ErrorKind::RateLimited,
        ErrorKind::QuotaExhausted,
        ErrorKind::AccountRestricted,
        ErrorKind::ChannelBlocked,
        ErrorKind::ModelNotFound,
    ] {
        for scope in [
            ErrorScope::Account,
            ErrorScope::Request,
            ErrorScope::AccountModel("glm-5.3".into()),
        ] {
            let error = GatewayError::new(kind, scope, 429, "unavailable", true);
            assert!(
                !should_preserve_affinity_after_error(Some(&binding), &bound, &error),
                "{kind:?}"
            );
        }
    }
}

#[test]
fn request_scoped_failure_keeps_binding_to_same_account() {
    let binding = test_binding("channel-a", "account-a");
    let bound = AccountKey::new("channel-a", "account-a");
    let error = GatewayError::invalid_request("bad request");

    assert!(should_preserve_affinity_after_error(
        Some(&binding),
        &bound,
        &error
    ));
}

#[test]
fn channel_block_on_bound_account_breaks_affinity_even_if_request_scoped() {
    let binding = test_binding("channel-a", "account-a");
    let bound = AccountKey::new("channel-a", "account-a");
    let error = GatewayError::new(
        ErrorKind::ChannelBlocked,
        ErrorScope::Request,
        403,
        "blocked",
        true,
    );

    assert!(!should_preserve_affinity_after_error(
        Some(&binding),
        &bound,
        &error
    ));
}

#[test]
fn successful_overflow_keeps_original_binding_target() {
    let binding = test_binding("channel-a", "account-a");
    let overflow = AccountKey::new("channel-b", "account-b");

    let kept = binding_after_success(true, Some(&binding), &overflow, "glm-5.3");
    assert_eq!(kept, binding);

    let migrated = binding_after_success(false, Some(&binding), &overflow, "glm-5.3");
    assert_eq!(migrated.channel_id, "channel-b");
    assert_eq!(migrated.uid, "account-b");
    assert_eq!(migrated.upstream_model, "glm-5.3");
}

#[test]
fn next_turn_returns_to_original_account_after_a_temporary_fallback() {
    use crate::gateway::{
        config::{AffinityConfig, ScoringConfig},
        core::matrix::ModelRoute,
        protocol::decode,
    };

    let request = decode::openai::decode_chat(&serde_json::json!({
        "model": "glm-5.3", "session_id": "conversation-1",
        "messages": [{"role": "user", "content": "opening"}],
    }))
    .unwrap();
    for error in [
        GatewayError::network("connection dropped"),
        GatewayError::upstream(503, "temporary outage"),
    ] {
        let now = now_ms();
        let store = AffinityStore::new(&AffinityConfig::default());
        let keys = AffinityStore::candidate_keys("key-1", &request.model, &request);
        let original_binding = test_binding("channel-a", "account-a");
        store.bind(keys[0].clone(), original_binding, now);
        let candidates: Vec<Candidate> = ["account-a", "account-b"]
            .iter()
            .map(|uid| {
                let mut account = PoolAccount::new("channel-a", *uid);
                account.requires_credential = false;
                Candidate {
                    account: Arc::new(account),
                    route: ModelRoute {
                        public_model: request.model.clone(),
                        channel_id: "channel-a".into(),
                        upstream_model: request.model.clone(),
                        weight: if *uid == "account-b" { 100 } else { 1 },
                        ..Default::default()
                    },
                }
            })
            .collect();
        let (_, binding) = store.lookup(&keys, now).unwrap();
        let original = router::affinity_pick(&candidates, &binding, &request.model).unwrap();
        assert_eq!(original.account.key.uid, "account-a");
        original.account.apply_error(
            &error,
            &crate::gateway::config::CooldownConfig::default(),
            now,
        );
        let keep =
            should_preserve_affinity_after_error(Some(&binding), &original.account.key, &error);
        let tried = HashSet::from([original.account.key.clone()]);
        let (backup, guard) =
            router::pick(&candidates, &tried, None, &ScoringConfig::default(), 1).unwrap();
        assert_eq!(backup.account.key.uid, "account-b");
        store.bind(
            keys[0].clone(),
            binding_after_success(keep, Some(&binding), &backup.account.key, &request.model),
            now,
        );
        drop(guard);
        let (_, next_binding) = store.lookup(&keys, now).unwrap();
        let next = router::affinity_pick(&candidates, &next_binding, &request.model).unwrap();
        assert!(next.account.is_ready(&request.model, now));
        assert_eq!(
            next.account.key.uid, "account-a",
            "备用号权重更高也不能接管原绑定"
        );
    }
}

#[test]
fn reset_bearing_rate_limit_is_narrowed_to_the_model() {
    let error = GatewayError::new(
        crate::gateway::errors::ErrorKind::RateLimited,
        ErrorScope::Account,
        429,
        "x",
        true,
    )
    .with_reset_at(Some(1_000));
    match narrow_scope(error, "glm-5.3").scope {
        ErrorScope::AccountModel(model) => assert_eq!(model, "glm-5.3"),
        other => panic!("应当收窄到模型级，实际 {other:?}"),
    }
}

#[test]
fn rate_limit_without_reset_stays_account_level() {
    let error = GatewayError::new(
        crate::gateway::errors::ErrorKind::RateLimited,
        ErrorScope::Account,
        429,
        "x",
        true,
    );
    assert!(matches!(
        narrow_scope(error, "glm-5.3").scope,
        ErrorScope::Account
    ));
}

#[test]
fn repeated_model_busy_errors_leave_account_and_models_ready() {
    use crate::gateway::{channels::workbuddy, config::CooldownConfig};

    let mut account = PoolAccount::new("workbuddy", "busy-account");
    account.requires_credential = false;
    let now = now_ms();
    // 多次繁忙也不能累计为账号软限流、模型额度冷却或 5xx 熔断。
    for body in [
        r#"{"code":14003,"retry_policy":{"retry":true,"retry_after":30}}"#,
        r#"{"code":14003}"#,
    ] {
        for _ in 0..10 {
            let error = narrow_scope(
                workbuddy::refine(429, body, "upstream-flash"),
                "upstream-flash",
            );
            assert_eq!(error.scope, ErrorScope::Request);
            account.apply_error(&error, &CooldownConfig::default(), now);
            assert!(account.is_ready("upstream-flash", now));
            assert!(account.is_ready("another-model", now));
            assert!(account.blocked_reasons("upstream-flash", now).is_empty());
            assert_eq!(account.cooldown_until_ms(), 0);
            assert!(account.model_cooldowns_snapshot(now).is_empty());
            let view = account.view(now);
            assert!(view.ready);
            assert!(view.blocked_reasons.is_empty());
            assert!(view.model_limits.is_empty());
        }
    }
    // 繁忙不应提前累积后续真实账号 429 的退避次数。
    let policy = CooldownConfig::default();
    account.apply_error(
        &workbuddy::refine(429, "too many requests", "upstream-flash"),
        &policy,
        now,
    );
    assert_eq!(
        account.cooldown_until_ms(),
        now + policy.soft_base_secs * 1000
    );
}
