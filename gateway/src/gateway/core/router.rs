//! 选号：候选展开 → 亲和短路 → 优先级分层 → 层内打分加权随机。
//!
//! 「聊天能不能用这个模型」由矩阵回答；「用哪个账号」由这里回答。
//! 打分把三股力量放在同一量纲上：缓存亲和（保命中）、负载（防单点打满）、成本（免费优先）。

use std::{collections::HashSet, hash::Hasher, sync::Arc};

use crate::gateway::{
    config::ScoringConfig,
    core::{
        affinity::Binding,
        matrix::{Matrix, ModelRoute, Requirements},
        pool::{AccountKey, AccountPool, InflightGuard, PoolAccount},
    },
    timeutil::now_ms,
};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub route: ModelRoute,
    pub account: Arc<PoolAccount>,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub route: ModelRoute,
    pub account: Arc<PoolAccount>,
    /// 是否来自亲和绑定（面板与决策日志要区分）。
    pub via_affinity: bool,
}

/// 候选展开：矩阵给出的候选渠道 × 该渠道的账号。
///
/// 返回（可用候选, 被剔除原因）—— 原因是给用户看的，本地 400 时直接进错误消息。
pub fn candidates(
    matrix: &Matrix,
    requirements: &Requirements,
    pool: &AccountPool,
    public_model: &str,
) -> CandidateSet {
    let mut set = CandidateSet::default();
    for route in matrix.routes(public_model) {
        // 被请求要求挡下的候选（不支持图片/工具、上下文超限）：属于**请求**问题，
        // 要报 400 并说明原因，不能混进"没账号"里报 503。
        if let Some(reason) = Matrix::reject_reason(&route, requirements) {
            set.rejected_requirements
                .push(format!("{}：{reason}", route.channel_id));
            continue;
        }
        let accounts = pool.by_channel(&route.channel_id);
        if accounts.is_empty() {
            set.channels_without_accounts.push(route.channel_id.clone());
            continue;
        }
        for account in accounts {
            if !account.enabled {
                continue;
            }
            // 账号级白名单：这个号不服务该模型就直接跳过（不是"请求有问题"，
            // 所以不进 rejected_requirements —— 那是给 400 用的）。
            if !account.model_allow.is_empty()
                && !account.model_allow.iter().any(|name| name == public_model)
            {
                continue;
            }
            // 账号级改名：同渠道不同账号可以把同一个对外模型映射到不同上游名。
            let mut route = route.clone();
            if let Some(upstream) = account.model_rename.get(public_model) {
                route.upstream_model = upstream.clone();
            }
            set.usable.push(Candidate {
                route,
                account,
            });
        }
    }
    set
}

/// 候选展开的结果：可用候选 + 两类被排除的原因（请求问题 / 没有账号）。
#[derive(Debug, Default)]
pub struct CandidateSet {
    pub usable: Vec<Candidate>,
    pub rejected_requirements: Vec<String>,
    pub channels_without_accounts: Vec<String>,
}

/// 亲和短路：绑定指向的账号仍在候选里就直接用。
pub fn affinity_pick(
    candidates: &[Candidate],
    binding: &Binding,
    public_model: &str,
) -> Option<Candidate> {
    candidates
        .iter()
        .find(|candidate| {
            candidate.account.key.channel_id == binding.channel_id
                && candidate.account.key.uid == binding.uid
                // 换了模型就换绑定：不同模型的候选集与缓存都不共享。
                && (binding.upstream_model.is_empty()
                    || candidate.route.upstream_model == binding.upstream_model
                    || matrix_model_matches(&candidate.route, public_model))
        })
        .cloned()
}

fn matrix_model_matches(route: &ModelRoute, public_model: &str) -> bool {
    route.public_model == public_model
}

/// 分层 + 层内打分加权随机。
///
/// 分层在前：priority 表达成本/质量档（免费档 > 付费档），跨档不做"偶尔选便宜的"。
/// 选号。可用性检查一律用**候选自己的 upstream 模型名** —— 模型级冷却就是按
/// upstream 名记的（写进池子的也是它），拿对外名去查会永远查不到，
/// 结果就是"明明限流了却还在选它、一直吃 429"。
pub fn pick(
    candidates: &[Candidate],
    tried: &HashSet<AccountKey>,
    hint: Option<&AccountKey>,
    scoring: &ScoringConfig,
    seed: u64,
) -> Option<(Candidate, InflightGuard)> {
    let now = now_ms();
    let live: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            !tried.contains(&candidate.account.key)
                && candidate
                    .account
                    .is_ready(&candidate.route.upstream_model, now)
                && !candidate.account.inflight_full()
        })
        .collect();
    if live.is_empty() {
        return None;
    }

    let top_priority = live
        .iter()
        .map(|candidate| candidate.route.priority)
        .max()
        .unwrap_or(0);
    let tier: Vec<(f64, &Candidate)> = live
        .iter()
        .filter(|candidate| candidate.route.priority == top_priority)
        .map(|candidate| (score(candidate, hint, scoring), *candidate))
        .collect();

    let chosen = weighted_pick(&tier, seed)?;
    // 并发下可能刚好被别人抢走最后一个槽位：返回 None 让调用方换号。
    let guard = chosen.account.try_acquire()?;
    Some((chosen.clone(), guard))
}

/// 打分：正数表示更该被选中。量纲统一在"权重 1.0 ≈ 一次完整缓存命中的偏好"附近。
pub fn score(candidate: &Candidate, hint: Option<&AccountKey>, scoring: &ScoringConfig) -> f64 {
    let mut value = candidate.route.weight.max(1) as f64;

    // 负载：在飞占满的账号要被推后，避免把同一账号打满。
    let limit = candidate.account.max_inflight().max(1) as f64;
    value -= (candidate.account.inflight() as f64 / limit) * scoring.load_weight;

    // 缓存亲和：同账号 > 同渠道 > 换渠道（换渠道必丢缓存，代价最高）。
    if let Some(hint) = hint {
        if hint == &candidate.account.key {
            value += scoring.affinity_bonus;
        } else if hint.channel_id == candidate.account.key.channel_id {
            value += scoring.affinity_bonus * 0.5;
        } else {
            value -= scoring.switch_penalty;
        }
    }

    // 成本：免费档优先（可把 cost_bonus 配成 0 关闭）。
    if candidate.route.is_free() {
        value += scoring.cost_bonus;
    }
    value
}

/// 平移分数到正区间后做轮盘抽样（等价于"无放回加权抽样的第一步"）。
fn weighted_pick<'a>(items: &[(f64, &'a Candidate)], seed: u64) -> Option<&'a Candidate> {
    let Some(minimum) = items
        .iter()
        .map(|(value, _)| *value)
        .fold(None::<f64>, |acc, value| {
            Some(acc.map_or(value, |current| current.min(value)))
        })
    else {
        return None;
    };
    let weights: Vec<f64> = items
        .iter()
        .map(|(value, _)| (value - minimum) + 1.0)
        .collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return items.first().map(|(_, candidate)| *candidate);
    }

    let mut roll = (jitter(seed) as f64 / u32::MAX as f64) * total;
    for (index, weight) in weights.iter().enumerate() {
        if roll < *weight {
            return Some(items[index].1);
        }
        roll -= weight;
    }
    items.last().map(|(_, candidate)| *candidate)
}

/// 会话内稳定、会话间分散的伪随机：同一会话重试时顺序不变。
pub fn seed_for(session: Option<&str>, model: &str, bucket_ms: u64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(session.unwrap_or("").as_bytes());
    hasher.write(model.as_bytes());
    // 无会话锚点时混入时间片，避免纯负载请求长期钉死同一账号。
    hasher.write_u64(if session.is_some() { 0 } else { bucket_ms / 30_000 });
    hasher.finish()
}

fn jitter(seed: u64) -> u32 {
    let mut state = seed | 1;
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    (state >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        channels::ModelCapabilities,
        config::{AffinityConfig, CooldownConfig},
        core::matrix::{ChannelCatalog, ModelLimits},
        errors::{ErrorKind, ErrorScope, GatewayError},
    };

    fn account(channel: &str, uid: &str, priority: u32, weight: u32) -> Arc<PoolAccount> {
        let mut account = PoolAccount::new(channel, uid);
        account.requires_credential = false;
        account.priority = priority;
        account.weight = weight;
        account.set_max_inflight(2);
        Arc::new(account)
    }

    fn candidate(account: Arc<PoolAccount>, public_model: &str, upstream: &str, priority: u32) -> Candidate {
        Candidate {
            route: ModelRoute {
                public_model: public_model.into(),
                channel_id: account.key.channel_id.clone(),
                upstream_model: upstream.into(),
                priority,
                weight: 1,
                enabled: true,
                credits: None,
                limits: ModelLimits::default(),
                capabilities: ModelCapabilities::default(),
                caps_source: crate::gateway::core::matrix::CapsSource::None,
                source: crate::gateway::core::matrix::RouteSource::Live,
                ..ModelRoute::default()
            },
            account,
        }
    }

    #[test]
    fn candidates_expand_per_channel_and_report_rejections() {
        let catalogs = vec![
            ChannelCatalog {
                channel_id: "wb-cn".into(),
                priority: 100,
                weight: 1,
                live: true,
                models: vec![crate::gateway::channels::UpstreamModel {
                    id: "glm-5.3".into(),
                    context_length: Some(1_000),
                    capabilities: ModelCapabilities {
                        vision: false,
                        tool_calls: false,
                        reasoning: true,
                        declared_tools: true,
                        declared_vision: true,
                    },
                    ..Default::default()
                }],
            },
            ChannelCatalog {
                channel_id: "qoder".into(),
                priority: 90,
                weight: 1,
                live: true,
                models: vec![crate::gateway::channels::UpstreamModel {
                    id: "glm-5.3".into(),
                    ..Default::default()
                }],
            },
        ];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);
        let pool = AccountPool::new(
            vec![account("wb-cn", "u1", 100, 1), account("qoder", "u2", 90, 1)],
            CooldownConfig::default(),
        );

        let set = candidates(
            &matrix,
            &Requirements {
                vision: true,
                tools: false,
                context: 0,
                max_output: None,
            },
            &pool,
            "glm-5.3",
        );
        // wb-cn 明确声明了不支持图片 → 被剔除；
        // qoder 的能力来源是 none（目录里一个能力位都没有）→ 乐观放行。
        assert_eq!(set.usable.len(), 1);
        assert_eq!(set.usable[0].account.key.channel_id, "qoder");
        assert_eq!(set.rejected_requirements.len(), 1);
        assert!(set.rejected_requirements[0].contains("wb-cn"));
        assert!(set.channels_without_accounts.is_empty());
    }

    #[test]
    fn oversized_max_tokens_is_clamped_not_rejected() {
        // 参考实现的实测结论：客户端发一个远超上限的 max_tokens 时，
        // 上游是静默钳制到真实上限，而不是报错 —— 所以这里必须放行（由调度层钳制）。
        let catalogs = vec![ChannelCatalog {
            channel_id: "wb-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![crate::gateway::channels::UpstreamModel {
                id: "deepseek-v4.1-flash".into(),
                max_output_tokens: Some(128_000),
                capabilities: ModelCapabilities {
                    vision: false,
                    tool_calls: true,
                    reasoning: true,
                    declared_tools: true,
                    declared_vision: true,
                },
                ..Default::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);
        let pool = AccountPool::new(
            vec![account("wb-cn", "u1", 100, 1)],
            CooldownConfig::default(),
        );
        let set = candidates(
            &matrix,
            &Requirements {
                vision: false,
                tools: false,
                context: 0,
                // 客户端常发整段上下文长度那种大值
                max_output: Some(1_000_000),
            },
            &pool,
            "deepseek-v4.1-flash",
        );
        assert_eq!(set.usable.len(), 1, "超限的 max_tokens 不该把候选剔掉");
        assert!(set.rejected_requirements.is_empty());
    }

    #[test]
    fn missing_accounts_are_reported_separately_from_request_rejections() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "qoder".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![crate::gateway::channels::UpstreamModel {
                id: "glm-5.3".into(),
                ..Default::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);
        // 池子里只有 wb-cn 的账号，qoder 没账号
        let pool = AccountPool::new(
            vec![account("wb-cn", "u1", 100, 1)],
            CooldownConfig::default(),
        );
        let set = candidates(&matrix, &Requirements::default(), &pool, "glm-5.3");
        assert!(set.usable.is_empty());
        assert!(set.rejected_requirements.is_empty(), "这不是请求问题");
        assert_eq!(set.channels_without_accounts, vec!["qoder".to_string()]);
    }

    #[test]
    fn affinity_pick_matches_the_bound_account_and_model() {
        let bound = account("wb-cn", "u1", 100, 1);
        let candidates = vec![
            candidate(account("wb-cn", "u2", 100, 1), "glm-5.3", "glm-5.3", 100),
            candidate(bound.clone(), "glm-5.3", "glm-5.3", 100),
        ];
        let binding = Binding {
            channel_id: "wb-cn".into(),
            uid: "u1".into(),
            upstream_model: "glm-5.3".into(),
            expires_at_ms: u64::MAX,
            hits: 0,
        };
        let picked = affinity_pick(&candidates, &binding, "glm-5.3").expect("命中绑定");
        assert_eq!(picked.account.key.uid, "u1");

        // 绑定指向的账号已经不在候选里（例如被禁用）→ 不命中，走正常挑选。
        let binding = Binding {
            uid: "u9".into(),
            ..binding
        };
        assert!(affinity_pick(&candidates, &binding, "glm-5.3").is_none());
    }


    /// 账号级白名单：不在名单里的模型不能落到这个号上；空名单 = 不限。
    #[test]
    fn account_model_allowlist_filters_candidates() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "wb-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![
                crate::gateway::channels::UpstreamModel {
                    id: "glm-5.3".into(),
                    ..Default::default()
                },
                crate::gateway::channels::UpstreamModel {
                    id: "kimi-k2.6".into(),
                    ..Default::default()
                },
            ],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);

        let mut limited = PoolAccount::new("wb-cn", "u1");
        limited.requires_credential = false;
        limited.model_allow = vec!["glm-5.3".into()];
        let pool = AccountPool::new(vec![Arc::new(limited)], CooldownConfig::default());

        // 名单里有的模型：能选出候选。
        let allowed = candidates(&matrix, &Requirements::default(), &pool, "glm-5.3");
        assert_eq!(allowed.usable.len(), 1);

        // 名单里没有的模型：该账号直接不参与，且**不算**请求被拒（那是 400 的语义）。
        let blocked = candidates(&matrix, &Requirements::default(), &pool, "kimi-k2.6");
        assert!(blocked.usable.is_empty(), "白名单外的模型不该落到这个号");
        assert!(
            blocked.rejected_requirements.is_empty(),
            "白名单过滤是账号侧限制，不该报成请求问题（否则会误报 400）"
        );
    }

    /// 空白名单 = 不限制：该渠道的所有模型都能用。
    #[test]
    fn empty_allowlist_means_every_model_is_allowed() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "wb-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![crate::gateway::channels::UpstreamModel {
                id: "glm-5.3".into(),
                ..Default::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);
        let mut open = PoolAccount::new("wb-cn", "u1");
        open.requires_credential = false;
        let pool = AccountPool::new(vec![Arc::new(open)], CooldownConfig::default());
        assert_eq!(
            candidates(&matrix, &Requirements::default(), &pool, "glm-5.3").usable.len(),
            1
        );
    }

    /// 账号级改名：对外名不变，但候选里的上游名要换成该账号映射的值。
    ///
    /// 这是 Trae 那种"同模型不同名"渠道能共用一个对外名的关键。
    #[test]
    fn account_model_rename_rewrites_the_upstream_name() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "trae-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![crate::gateway::channels::UpstreamModel {
                id: "DeepSeek-V4-Flash-Official".into(),
                ..Default::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);

        let mut account = PoolAccount::new("trae-cn", "u1");
        account.requires_credential = false;
        // 对外统一叫 deepseek-v4.1-flash，落到这个号时换成 Trae 的上游名。
        account.model_rename.insert(
            "deepseek-v4.1-flash".into(),
            "DeepSeek-V4-Flash-Official".into(),
        );
        let pool = AccountPool::new(vec![Arc::new(account)], CooldownConfig::default());

        // 矩阵里这个对外名不存在，改名本身不创造候选 —— 所以这里用上游名建矩阵，
        // 再验证"名字一样时才改写"的行为不误伤。
        let same = candidates(
            &matrix,
            &Requirements::default(),
            &pool,
            "DeepSeek-V4-Flash-Official",
        );
        assert_eq!(same.usable.len(), 1);
        assert_eq!(
            same.usable[0].route.upstream_model, "DeepSeek-V4-Flash-Official",
            "没有对应改名条目时上游名保持原样"
        );
    }

    /// 改名命中时，候选的 upstream_model 必须是映射后的值（出站就发这个）。
    #[test]
    fn rename_applies_when_the_public_name_matches() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "trae-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![crate::gateway::channels::UpstreamModel {
                id: "deepseek-v4.1-flash".into(),
                ..Default::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &std::collections::HashMap::new(), &[]);

        let mut account = PoolAccount::new("trae-cn", "u1");
        account.requires_credential = false;
        account.model_rename.insert(
            "deepseek-v4.1-flash".into(),
            "DeepSeek-V4-Flash-Official".into(),
        );
        let pool = AccountPool::new(vec![Arc::new(account)], CooldownConfig::default());

        let set = candidates(
            &matrix,
            &Requirements::default(),
            &pool,
            "deepseek-v4.1-flash",
        );
        assert_eq!(set.usable.len(), 1);
        assert_eq!(
            set.usable[0].route.upstream_model, "DeepSeek-V4-Flash-Official",
            "出站必须用该账号映射的上游名"
        );
        assert_eq!(
            set.usable[0].route.public_model, "deepseek-v4.1-flash",
            "对外名不能被改写"
        );
    }
    #[test]
    fn pick_stays_in_the_highest_priority_tier_and_skips_tried() {
        let free = candidate(account("wb-cn", "free", 100, 1), "glm-5.3", "glm-5.3", 100);
        let paid = candidate(account("qoder", "paid", 50, 1), "glm-5.3", "glm-5.3", 50);
        let candidates = vec![free.clone(), paid];
        let scoring = ScoringConfig::default();

        for seed in 0..20u64 {
            let (picked, guard) = pick(&candidates, &HashSet::new(), None, &scoring, seed * 7919)
            .expect("有候选");
            assert_eq!(picked.route.priority, 100, "不该跨档选到付费渠道");
            drop(guard);
        }

        // 已试过最高档 → 落到下一档，而不是返回 None。
        let mut tried = HashSet::new();
        tried.insert(free.account.key.clone());
        let (picked, _guard) = pick(&candidates, &tried, None, &scoring, 1).expect("下一档");
        assert_eq!(picked.route.priority, 50);
    }

    /// 模型级限流必须真的影响选号：写冷却用 upstream 名，查的时候也得用同一个名字。
    /// （早先一边用对外名、一边用 upstream 名，冷却记了却查不到，账号继续吃 429。）
    #[test]
    fn model_park_hides_that_model_but_not_siblings() {
        let account = account("wb-cn", "u1", 100, 1);
        account.apply_error(
            &GatewayError::new(
                ErrorKind::RateLimited,
                ErrorScope::AccountModel("glm-5.3-upstream".into()),
                429,
                "该模型被上游限流",
                true,
            ),
            &CooldownConfig::default(),
            now_ms(),
        );
        let parked = candidate(
            Arc::clone(&account),
            "glm-5.3",
            "glm-5.3-upstream",
            100,
        );
        let sibling = candidate(Arc::clone(&account), "deepseek-v4.1-flash", "ds-upstream", 100);
        let scoring = ScoringConfig::default();

        assert!(
            pick(&[parked], &HashSet::new(), None, &scoring, 1).is_none(),
            "被限流的模型不能再被选中"
        );
        let (picked, _guard) = pick(&[sibling], &HashSet::new(), None, &scoring, 1)
            .expect("同账号的其他模型照常可用");
        assert_eq!(picked.route.upstream_model, "ds-upstream");
    }

    #[test]
    fn affinity_hint_beats_load_and_switching_costs() {
        let scoring = ScoringConfig::default();
        let hinted = candidate(account("wb-cn", "u1", 100, 1), "glm-5.3", "glm-5.3", 100);
        let other_channel = candidate(account("qoder", "u9", 100, 1), "glm-5.3", "glm-5.3", 100);
        let hint = hinted.account.key.clone();

        let hinted_score = score(&hinted, Some(&hint), &scoring);
        let switched_score = score(&other_channel, Some(&hint), &scoring);
        assert!(
            hinted_score > switched_score,
            "同账号应当比换渠道分高：{hinted_score} vs {switched_score}"
        );

        // 同渠道换号：比同账号差，但比换渠道好。
        let same_channel = candidate(account("wb-cn", "u2", 100, 1), "glm-5.3", "glm-5.3", 100);
        let same_channel_score = score(&same_channel, Some(&hint), &scoring);
        assert!(hinted_score > same_channel_score);
        assert!(same_channel_score > switched_score);
    }

    #[test]
    fn free_routes_get_a_cost_bonus() {
        let mut free = candidate(account("wb-cn", "u1", 100, 1), "glm-5.3", "glm-5.3", 100);
        free.route.credits = Some("x0.00".into());
        let paid = candidate(account("qoder", "u2", 100, 1), "glm-5.3", "glm-5.3", 100);
        let scoring = ScoringConfig::default();
        assert!(score(&free, None, &scoring) > score(&paid, None, &scoring));
    }

    #[test]
    fn seed_is_stable_within_a_session_and_varies_without_one() {
        let a = seed_for(Some("conv-1"), "glm-5.3", 1_000);
        let b = seed_for(Some("conv-1"), "glm-5.3", 999_999);
        assert_eq!(a, b, "同会话重试的顺序必须稳定");
        let c = seed_for(None, "glm-5.3", 0);
        let d = seed_for(None, "glm-5.3", 60_000);
        assert_ne!(c, d, "无锚点的纯负载请求要打散");
    }

    #[test]
    fn all_accounts_cooling_yields_none() {
        let mut poisoned = account("wb-cn", "u1", 100, 1);
        Arc::get_mut(&mut poisoned)
            .expect("独占")
            .apply_error(
                &crate::gateway::errors::GatewayError::new(
                    crate::gateway::errors::ErrorKind::AuthExpired,
                    crate::gateway::errors::ErrorScope::Account,
                    401,
                    "x",
                    true,
                ),
                &CooldownConfig::default(),
                now_ms(),
            );
        let candidates = vec![candidate(poisoned, "glm-5.3", "glm-5.3", 100)];
        assert!(pick(
            &candidates,
            &HashSet::new(),
            None,
            &ScoringConfig::default(),
            1
        )
        .is_none());
        let _ = AffinityConfig::default();
    }
}
