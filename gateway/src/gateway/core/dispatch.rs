//! 调度：候选矩阵 → 亲和短路 → 分层打分选号 → 重试排除 → 冷却落地 → 并发槽。
//!
//! 这一层是「多号池」与「多渠道」的汇合点：候选来自矩阵，挑选只认账号运行态与统一错误语义。

use std::{collections::HashSet, sync::Arc};

use uuid::Uuid;

use crate::gateway::{
    channels::ChannelAdapter,
    core::{
        affinity::{AffinityMode, AffinityStore, Binding},
        matrix::Requirements,
        pool::{AccountKey, InflightGuard, PoolAccount},
        refresh,
        router::{self, Candidate, Selection},
        usage::estimate_request_tokens,
    },
    errors::{ErrorKind, ErrorScope, GatewayError},
    protocol::cir::CanonicalRequest,
    state::GatewayState,
    timeutil::now_ms,
    transport::FrameStream,
};

/// 一次已经连上上游的会话。持有账号在飞槽位，`drop` 即归还。
pub struct UpstreamSession {
    pub selection: Selection,
    pub adapter: Arc<dyn ChannelAdapter>,
    pub frames: FrameStream,
    pub attempts: usize,
    pub trace_id: String,
    /// 本次请求的亲和键（成功后写回/续期）。
    pub affinity_key: Option<String>,
    _guard: InflightGuard,
}

impl UpstreamSession {
    pub fn account(&self) -> &Arc<PoolAccount> {
        &self.selection.account
    }
}

/// 选号并连上上游；失败自动换号重试，全部失败返回最后一次的错误。
///
/// `key` 是发起请求的 Key 权限上下文。渠道与模型的过滤在**建候选集阶段**完成
/// （SPEC 12.2）：选完号再检查会出现"选到 → 拒绝 → 重试 → 再选到同一个"的死循环，
/// 而且并发槽已经被占过一轮。
pub async fn open_chat_session(
    state: &GatewayState,
    request: &CanonicalRequest,
    key: &crate::gateway::keyauth::KeyContext,
) -> Result<UpstreamSession, GatewayError> {
    let trace_id = short_id();
    let scope = key.scope.clone();

    // ① 候选：模型 × 渠道矩阵 + 请求级硬性要求过滤（本地就把不可能的发包剔掉）。
    let matrix = state.ensure_matrix(false).await;
    if matrix.routes(&request.model).is_empty() {
        // 没有任何渠道提供这个模型：这是调用方的问题（名字写错 / 没配覆盖），本地 400。
        return Err(GatewayError::invalid_request(format!(
            "没有渠道提供模型 {}：检查模型名，或用 aliases / routes 配置这条路由",
            request.model
        )));
    }

    // Key 的模型白名单：模型存在但该 Key 用不了 → 403（不是 503）。
    if !key.allows_model(&request.model) {
        return Err(GatewayError::key_forbidden(format!(
            "该 Key 无权使用模型 {}：允许的模型是 {}",
            request.model,
            describe_allowlist(&key.model_allow)
        )));
    }

    let requirements = Requirements::from_request(request, estimate_request_tokens(request));
    let mut set = router::candidates(&matrix, &requirements, &state.pool, &request.model);

    // Key 的渠道白名单：在建集阶段剔除，而不是选号之后再拒绝。
    let before = set.usable.len();
    set.usable
        .retain(|candidate| key.allows_channel(&candidate.route.channel_id));
    let filtered_by_key = before - set.usable.len();

    let candidates = set.usable;
    let rejected: Vec<String> = set
        .rejected_requirements
        .iter()
        .cloned()
        .chain(
            set.channels_without_accounts
                .iter()
                .map(|channel| format!("{channel}：没有账号")),
        )
        .collect();
    if candidates.is_empty() {
        // 全被 Key 权限挡下：这是**权限**问题，不是容量问题。
        if filtered_by_key > 0 {
            return Err(GatewayError::key_forbidden(format!(
                "该 Key 无权使用模型 {}：允许的渠道（{}）都不提供它",
                request.model,
                describe_allowlist(&key.channel_allow)
            )));
        }
        // 全被"请求要求"挡下（不支持图片/工具、上下文超限）：是请求的问题，报 400 说清原因。
        if !set.rejected_requirements.is_empty() {
            return Err(GatewayError::invalid_request(format!(
                "模型 {} 没有满足本次请求要求的渠道（{}）",
                request.model,
                set.rejected_requirements.join("；")
            )));
        }
        // 否则是容量问题：503 让调用方稍后重试。
        let detail = if set.channels_without_accounts.is_empty() {
            "先给渠道添加账号".to_string()
        } else {
            format!("{} 没有账号", set.channels_without_accounts.join("、"))
        };
        return Err(GatewayError::no_account(format!(
            "模型 {} 当前没有可用账号（{detail}）",
            request.model
        )));
    }

    // ② 亲和：命中且可用就直接用；不可用则阶梯式退化（同渠道 > 跨渠道），并清掉废绑定。
    let keys = AffinityStore::candidate_keys(&scope, &request.model, request);
    let mut hint: Option<AccountKey> = None;
    let mut affinity_key: Option<String> = None;
    let mut affinity_binding: Option<Binding> = None;
    let mut planned: Option<Candidate> = None;
    let mut via_affinity = false;
    // Keep a healthy binding across one-request capacity spillovers. The request
    // may use another account, but that should not migrate the whole session.
    let mut preserve_affinity = false;

    if state.affinity.enabled() {
        if let Some((key, binding)) = state.affinity.lookup(&keys, now_ms()) {
            hint = Some(AccountKey::new(
                binding.channel_id.clone(),
                binding.uid.clone(),
            ));
            if let Some(candidate) = router::affinity_pick(&candidates, &binding, &request.model) {
                if candidate
                    .account
                    .is_ready(&candidate.route.upstream_model, now_ms())
                {
                    affinity_key = Some(key.clone());
                    affinity_binding = Some(binding.clone());
                    if candidate.account.inflight_full() {
                        preserve_affinity = state.affinity.mode() == AffinityMode::Prefer;
                    } else {
                        planned = Some(candidate);
                    }
                }
            }
            if planned.is_none() && !preserve_affinity {
                if state.affinity.mode() == AffinityMode::Strict {
                    return Err(GatewayError::no_account(
                        "会话绑定当前不可用（strict 模式：宁可直接失败，也不换号重写缓存）",
                    ));
                }
                state.affinity.clear(&key);
                let _ = state.storage.delete_affinity(&key);
            }
        }
    }

    // ③ 选号 + 重试。tried 保证一轮请求内绝不重复选同一账号。
    let policy = state.pool.policy();
    let scoring = state.config().scoring;
    let session_seed = router::seed_for(
        request.metadata.session_key().as_deref(),
        &request.model,
        now_ms(),
    );
    let max_attempts = candidates.len().max(2) + 1;
    let mut tried: HashSet<AccountKey> = HashSet::new();
    let mut attempts = 0usize;
    let mut last_error: Option<GatewayError> = None;
    // 本轮已经因为 401 强制刷新过凭据的账号：每个号只刷一次，
    // 避免"刷新完 token 又被吊销"的号把刷新端点打成热点。
    let mut refreshed_once: HashSet<AccountKey> = HashSet::new();

    // 攒下"每个候选为什么没被选中"：全池失败时要把真实原因告诉调用方，
    // 而不是笼统地说"冷却或并发已满"（那会误导排查）。
    let mut skip_reasons: Vec<String> = Vec::new();

    while attempts < max_attempts {
        let (candidate, guard) = match planned.take() {
            Some(candidate) => {
                // 亲和目标：槽位被抢走就退回常规挑选，不排队等它。
                match candidate.account.try_acquire() {
                    Some(guard) => {
                        via_affinity = true;
                        (candidate, guard)
                    }
                    None => {
                        if state.affinity.mode() == AffinityMode::Strict {
                            return Err(GatewayError::no_account(
                                "会话绑定账号并发已满（strict 模式：宁可直接失败，也不换号重写缓存）",
                            ));
                        }
                        preserve_affinity = true;
                        continue;
                    }
                }
            }
            None => {
                via_affinity = false;
                let Some(picked) = router::pick(
                    &candidates,
                    &tried,
                    hint.as_ref(),
                    &scoring,
                    session_seed.wrapping_add(attempts as u64),
                ) else {
                    // 记下每个候选为什么不可用（冷却 / 熔断 / 额度停放 / 并发满）。
                    for candidate in &candidates {
                        if tried.contains(&candidate.account.key) {
                            continue;
                        }
                        let reasons = candidate
                            .account
                            .blocked_reasons(&candidate.route.upstream_model, now_ms());
                        let detail = if reasons.is_empty() {
                            if candidate.account.inflight_full() {
                                format!(
                                    "{}/{}：并发已满（{}/{}）",
                                    candidate.route.channel_id,
                                    candidate.account.key.uid,
                                    candidate.account.inflight(),
                                    candidate.account.max_inflight()
                                )
                            } else {
                                continue;
                            }
                        } else {
                            format!(
                                "{}/{}：{}",
                                candidate.route.channel_id,
                                candidate.account.key.uid,
                                reasons.join("；")
                            )
                        };
                        if !skip_reasons.contains(&detail) {
                            skip_reasons.push(detail);
                        }
                    }
                    break;
                };
                picked
            }
        };

        tried.insert(candidate.account.key.clone());
        attempts += 1;
        if attempts > 1 {
            state
                .stats
                .retries
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        match try_open(state, &candidate, request).await {
            Ok((adapter, frames)) => {
                // 成功要按 upstream 名清冷却：模型级冷却的 key 就是这个。
                candidate
                    .account
                    .on_success(&candidate.route.upstream_model);
                let previous_affinity_key = affinity_key.clone();
                let next_affinity_key = keys.first().cloned();
                let binding = binding_after_success(
                    preserve_affinity,
                    affinity_binding.as_ref(),
                    &candidate.account.key,
                    &candidate.route.upstream_model,
                );
                let target_key = if preserve_affinity {
                    next_affinity_key.or(previous_affinity_key.clone())
                } else {
                    next_affinity_key
                };
                let affinity_key = match (state.affinity.enabled(), target_key) {
                    (true, Some(key)) => {
                        let stored = state.affinity.bind(key.clone(), binding, now_ms());
                        // 命中也要把续期写回存储；内容指纹会滚到当前最新前缀。
                        if let Some(stored) = stored {
                            if let Err(error) = state.storage.save_affinity(&key, &stored) {
                                eprintln!("[gateway] 亲和绑定落库失败：{}", error.message);
                            } else if let Some(previous_key) = previous_affinity_key.as_deref() {
                                if previous_key != key {
                                    state.affinity.clear(previous_key);
                                    let _ = state.storage.delete_affinity(previous_key);
                                }
                            }
                        }
                        Some(key)
                    }
                    _ => None,
                };

                eprintln!(
                    "[gateway] trace={trace_id} ok account={} upstream={} attempts={attempts} affinity={} {}",
                    candidate.account.key,
                    candidate.route.upstream_model,
                    affinity_key.is_some(),
                    request.trace_summary()
                );
                state.push_decision(decision_entry(
                    &trace_id,
                    request,
                    candidates.len(),
                    Some(&candidate),
                    attempts,
                    via_affinity,
                    &rejected,
                    None,
                ));

                return Ok(UpstreamSession {
                    selection: Selection {
                        route: candidate.route,
                        account: candidate.account,
                        via_affinity,
                    },
                    adapter,
                    frames,
                    attempts,
                    trace_id,
                    affinity_key,
                    _guard: guard,
                });
            }
            Err(error) => {
                /*
                  上游说凭据不认（401）时，先**强制刷新一次**再重试同一个号。

                  为什么必须有这一步：选号路径的保鲜只在凭据"临期 2 分钟"时才刷新
                  （`refresh::ensure_credential`），但上游会因为风控 / 改密 / 异地登录
                  **提前吊销** token —— 本地 `expires_at_ms` 还没到，于是每轮探活、
                  每个请求都拿着同一个已经作废的 token 打上游，稳定得到 401，
                  而 refresh_token 明明还有效、一次刷新就能自愈。

                  只在 `Account` 范围内做：模型级 401 与调用方权限无关，
                  刷新换不出新权限。`refreshed_once` 保证每个号每轮只刷一次，
                  否则一个反复被吊销的号会把刷新端点打成热点。
                */
                let mut error = error;
                if error.kind == ErrorKind::AuthExpired
                    && matches!(error.scope, ErrorScope::Account)
                    && !refreshed_once.contains(&candidate.account.key)
                {
                    refreshed_once.insert(candidate.account.key.clone());
                    match refresh::refresh(state, &candidate.account, true).await {
                        Ok(_) => {
                            eprintln!(
                                "[gateway] trace={trace_id} 上游 401，已强制刷新凭据并重试 {}",
                                candidate.account.key
                            );
                            // 重试同一个号：先把槽位归还再走下一轮挑选。
                            drop(guard);
                            attempts -= 1;
                            tried.remove(&candidate.account.key);
                            continue;
                        }
                        Err(refresh_error) => {
                            // 刷新失败说明真的得重新登录：把刷新失败的原因带上，
                            // 否则界面上只剩一句"401"，看不出是哪一环断了。
                            error = GatewayError::new(
                                error.kind,
                                error.scope,
                                error.status,
                                format!(
                                    "{}；强制刷新凭据失败：{}",
                                    error.message, refresh_error.message
                                ),
                                error.retryable,
                            );
                        }
                    }
                }
                // 收窄范围时也用 upstream 名 —— 池子里的模型冷却就是按它记的。
                let error = narrow_scope(error, &candidate.route.upstream_model);
                candidate.account.apply_error(&error, &policy, now_ms());
                eprintln!(
                    "[gateway] trace={trace_id} fail account={} kind={:?} retryable={} {}",
                    candidate.account.key, error.kind, error.retryable, error.message
                );
                let retryable = error.retryable;
                // 渠道校验拦截（11-128）：换号重试最稳 —— 出站身份跟着账号变，
                // 同一个身份连发被拦的概率更高。
                let switch_account = error.kind == ErrorKind::ChannelBlocked;
                // 请求级错误（如内容拒绝、模型瞬时繁忙）不说明账号坏了；允许本次
                // 请求溢出到其他账号，但保留原会话绑定。账号/模型级故障和渠道拦截
                // 才解绑，让成功的备用账号接管后续请求。
                let keep_original_binding = should_preserve_affinity_after_error(
                    affinity_binding.as_ref(),
                    &candidate.account.key,
                    &error,
                );
                if let Some(key) = affinity_key.as_ref() {
                    if keep_original_binding {
                        preserve_affinity = true;
                    } else {
                        state.affinity.clear(key);
                        let _ = state.storage.delete_affinity(key);
                        affinity_key = None;
                        affinity_binding = None;
                        preserve_affinity = false;
                    }
                }
                last_error = Some(error);
                // 先归还槽位再换号，否则这个账号在本轮请求里再也选不到。
                drop(guard);
                if !retryable || (state.affinity.mode() == AffinityMode::Strict && via_affinity) {
                    break;
                }
                if switch_account {
                    // 强制换号：把"亲和指回同一账号"的捷径跳过。
                    hint = None;
                }
                // P2：403 内容审核要在这里用降级提示词原号重试一次（换号无效）。
            }
        }
    }

    let error = last_error.unwrap_or_else(|| {
        if skip_reasons.is_empty() {
            // 没有任何一个候选落到这里：说明它们全被排除在候选集之外了。
            GatewayError::no_account(format!(
                "模型 {} 当前没有可用账号（本次请求的候选已被排除或已尝试）",
                request.model
            ))
        } else {
            // 说真话：把"每个账号为什么不可用"直接给出来。
            let mut message = format!("模型 {} 当前没有可用账号：", request.model);
            for (index, reason) in skip_reasons.iter().take(4).enumerate() {
                if index > 0 {
                    message.push_str("；");
                }
                message.push_str(reason);
            }
            if skip_reasons.len() > 4 {
                message.push_str(&format!("；等共 {} 个", skip_reasons.len()));
            }
            GatewayError::no_account(message)
        }
    });
    state.push_decision(decision_entry(
        &trace_id,
        request,
        candidates.len(),
        None,
        attempts,
        via_affinity,
        &rejected,
        Some(&error),
    ));
    Err(error)
}

/// 连一次上游：先保鲜凭据，再按渠道构建报文并取帧流。
async fn try_open(
    state: &GatewayState,
    candidate: &Candidate,
    request: &CanonicalRequest,
) -> Result<(Arc<dyn ChannelAdapter>, FrameStream), GatewayError> {
    if !refresh::ensure_credential(state, &candidate.account).await {
        return Err(GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            "凭据刷新失败，需要重新登录",
            true,
        ));
    }
    // 适配器按**账号的平台**选，不是按渠道。
    // 平台下沉到账号后，同一渠道下可能混着 WorkBuddy 与自定义供应商的号，
    // 用渠道 id 去查注册表会查不到（或查到别人的）。
    let adapter = state.adapter_for(&candidate.account).ok_or_else(|| {
        GatewayError::internal(format!(
            "账号 {}/{} 没有平台配置，无法选择适配器",
            candidate.account.key.channel_id, candidate.account.key.uid
        ))
    })?;

    // 上游模型名可能与对外名不同（矩阵的 upstream_model 就是干这个的）。
    let mut routed = request.clone();
    routed.model = candidate.route.upstream_model.clone();
    // 客户端常发一个远超上限的 max_tokens：上游是静默钳制而不是报错，
    // 网关照做（按该渠道目录里的上限钳），免得白跑一趟 400。
    if let (Some(limit), Some(requested)) = (candidate.route.limits.max_output, routed.max_tokens) {
        if limit > 0 && requested > limit {
            routed.max_tokens = Some(limit);
        }
    }

    // **按账号取客户端**：绑了代理就走代理的出口 IP，没绑才用共享直连。
    // 这里不能直接用 `state.client` —— 那会让"绑了代理"完全无效，
    // 而且错得很安静（请求照样成功，只是 IP 没换）。
    let client = state.client_for(&candidate.account)?;
    let frames = adapter
        .chat_stream(&candidate.account.snapshot(), &routed, &client)
        .await?;
    Ok((adapter, frames))
}

/// 带 reset 时刻的 429 上游语义是「只限制这个模型」，把 scope 收窄到模型级，
/// 避免因为一个模型限流把整个账号（含其他模型）拉黑。
/// 通用收窄规则：带恢复时刻的账号级 429 视为"只限这个模型"（该上游家族的形态）。
///
/// 渠道知道得更准的（例如 workbuddy 的 6004）会在 `classify_error` 里直接给出
/// [`ErrorScope::AccountModel`]，不依赖这条兜底规则。
fn narrow_scope(mut error: GatewayError, model: &str) -> GatewayError {
    if error.reset_at_ms.is_some() && matches!(error.scope, ErrorScope::Account) {
        error.scope = ErrorScope::AccountModel(model.to_string());
    }
    error
}

/// Keep a binding when an overflow account fails, or when the failure only
/// describes this request. Failures on the bound account itself break affinity
/// only when their scope says the account or its model is unhealthy.
fn should_preserve_affinity_after_error(
    binding: Option<&Binding>,
    attempted_account: &AccountKey,
    error: &GatewayError,
) -> bool {
    let Some(binding) = binding else {
        return false;
    };
    let attempted_bound_account =
        binding.channel_id == attempted_account.channel_id && binding.uid == attempted_account.uid;
    !attempted_bound_account
        || (matches!(&error.scope, ErrorScope::Request)
            && !matches!(
                error.kind,
                ErrorKind::AuthExpired | ErrorKind::ChannelBlocked
            ))
}

/// On capacity spillover, advance the content-prefix key without moving the
/// account binding to the temporary account used for this single request.
fn binding_after_success(
    preserve_affinity: bool,
    previous_binding: Option<&Binding>,
    selected_account: &AccountKey,
    upstream_model: &str,
) -> Binding {
    if preserve_affinity {
        if let Some(binding) = previous_binding {
            return binding.clone();
        }
    }
    Binding {
        channel_id: selected_account.channel_id.clone(),
        uid: selected_account.uid.clone(),
        upstream_model: upstream_model.to_string(),
        expires_at_ms: 0,
        hits: 0,
    }
}

fn decision_entry(
    trace_id: &str,
    request: &CanonicalRequest,
    candidate_count: usize,
    chosen: Option<&Candidate>,
    attempts: usize,
    via_affinity: bool,
    rejected: &[String],
    error: Option<&GatewayError>,
) -> serde_json::Value {
    serde_json::json!({
        "trace": trace_id,
        "ts": now_ms(),
        "model": request.model,
        "stream": request.stream,
        "session": request.metadata.session_key(),
        "candidates": candidate_count,
        "rejected": rejected,
        "attempts": attempts,
        "via_affinity": via_affinity,
        "channel": chosen.map(|candidate| candidate.route.channel_id.clone()),
        "account": chosen.map(|candidate| candidate.account.key.to_string()),
        "upstream_model": chosen.map(|candidate| candidate.route.upstream_model.clone()),
        "error": error.map(|error| serde_json::json!({
            "kind": error.kind,
            "message": error.message,
        })),
    })
}

fn short_id() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// 把白名单格式化成人类可读的一行（用于 403 的错误消息）。
///
/// 空集合表示"不限"，但只有在**另一维**被挡时才会走到这里，
/// 所以空集合要显式说成"未限制"，避免出现"允许的渠道是（）"这种空括号。
fn describe_allowlist(list: &std::collections::HashSet<String>) -> String {
    if list.is_empty() {
        return "未限制".to_string();
    }

    let mut items: Vec<&str> = list.iter().map(String::as_str).collect();
    items.sort_unstable();
    items.join("、")
}

#[cfg(test)]
mod tests {
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
    fn account_failure_on_bound_account_breaks_affinity() {
        let binding = test_binding("channel-a", "account-a");
        let bound = AccountKey::new("channel-a", "account-a");
        let error = GatewayError::network("temporary upstream failure");

        assert!(!should_preserve_affinity_after_error(
            Some(&binding),
            &bound,
            &error
        ));
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
}

/// 单账号真实探针：只对该账号发一条最小请求，返回耗时与回复摘要。
///
/// 与参考实现的 `/accounts/test` 同口径：**真实打上游**（不是测端口通不通），
/// 成功即清掉该账号的错误状态，失败按统一语义记冷却。
/// 指定 `model` 时用指定模型，否则用该渠道第一个可用模型。
pub async fn probe_account(
    state: &GatewayState,
    account: &Arc<PoolAccount>,
    model: Option<&str>,
    timeout: std::time::Duration,
) -> Result<ProbeOutcome, GatewayError> {
    let started = std::time::Instant::now();
    let adapter = state
        .adapter_for(account)
        .ok_or_else(|| GatewayError::internal("账号没有平台配置，无法探活"))?;

    // 选模型：显式指定 > 矩阵里第一个属于该渠道的模型。
    let matrix = state.ensure_matrix(false).await;
    // 默认选**免费**模型（探针会真实消耗积分，不该拿付费档试）；
    // 没有免费的就退回该渠道的第一个可用模型。
    let belongs_to_channel = |model: &str| {
        matrix
            .routes(model)
            .iter()
            .any(|route| route.channel_id == account.key.channel_id)
    };
    let chosen = match model {
        Some(model) => model.to_string(),
        None => {
            let candidates: Vec<String> = matrix
                .public_models()
                .into_iter()
                .filter(|model| belongs_to_channel(model))
                .collect();
            /*
              挑一个"尽量便宜"的模型来探活。

              先找上游明确标了免费的（`credits=free`）—— 实测多数渠道从不标这个，
              所以还要有第二档：按名字里的轻量关键词挑（flash / mini / nano / lite / turbo），
              这类模型单价通常最低。

              都没有时才退回第一个候选。**不能只靠 `is_free()`**：
              上游不标 free 时它会一直落空，最后"第一个"是什么取决于列表顺序，
              于是同一个分组每次探活可能用不同模型，额度消耗不可预期。
            */
            /*
              挑一个"尽量便宜"的模型来探活。

              三档，按优先级：
                1. 上游明确标了 `credits=free` 的（实测多数渠道从不标）；
                2. 名字带 flash / mini / nano / lite / turbo 的轻量档（单价通常最低）；
                3. 兜底取第一个候选。

              **写成显式 if-else 而不是 `or_else` 链**：链式写法里
              `find(|m| ...).cloned()` 的闭包参数是 `&&String`，
              类型推断稍有偏差就会静默落到最后一档（实测就是如此 ——
              轻量命中明明是 deepseek-v4.1-flash，最终却选了别的）。
              显式的分支读起来也更容易确认每一档真的会生效。
            */
            let light_keywords = ["flash", "mini", "nano", "lite", "turbo", "air"];
            let is_light = |model: &str| {
                let name = model.to_ascii_lowercase();
                light_keywords.iter().any(|word| name.contains(word))
            };

            /*
              只按"轻量档"挑，**不看 `is_free()`**。

              为什么去掉 free 这一档：`is_free()` 依据上游的 `credits` 字段
              （`"x0.00"` / `"0"` 之类），而那个字段**不通过 catalog 接口透出** ——
              结果是"网关认为某个模型免费、界面完全看不出来"，
              用户会以为系统在乱选（实测 hy3 就是这样被选中的）。

              轻量关键词是可见、可解释的：名字里有 flash / mini / nano 的那类，
              单价通常最低。挑错了用户也能自己指定。
            */
            let light_pick: Option<String> = candidates
                .iter()
                .find(|model| is_light(model.as_str()))
                .cloned();

            light_pick
                .or_else(|| candidates.first().cloned())
                .ok_or_else(|| GatewayError::invalid_request("该渠道没有任何模型可用于测试"))?
        }
    };
    let upstream_model = matrix
        .routes(&chosen)
        .into_iter()
        .find(|route| route.channel_id == account.key.channel_id)
        .map(|route| route.upstream_model)
        .unwrap_or_else(|| chosen.clone());

    // 最小请求：一句 hi，非流式由本地折叠。
    let request = CanonicalRequest {
        model: upstream_model.clone(),
        system: Vec::new(),
        messages: vec![crate::gateway::protocol::cir::Message {
            role: crate::gateway::protocol::cir::Role::User,
            content: vec![crate::gateway::protocol::cir::Block::text("hi")],
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            reasoning: None,
        }],
        tools: Vec::new(),
        tool_choice: None,
        temperature: None,
        top_p: None,
        max_tokens: Some(64),
        stream: false,
        reasoning: None,
        metadata: Default::default(),
        raw: serde_json::json!({}),
    };

    if !refresh::ensure_credential(state, account).await {
        return Err(GatewayError::new(
            ErrorKind::AuthExpired,
            ErrorScope::Account,
            401,
            "凭据不可用，需要重新登录",
            false,
        ));
    }

    // 用闭包而不是直接保存 future：401 之后要**再跑一次同样的探针**，
    // 而 async 块只能被 await 一次（move 之后再使用时编译器会报 E0382）。
    let attempt = || async {
        // 同上：探活也要按账号走代理，否则"绑了代理的号"探活结果代表不了它真实的出口。
        let client = state.client_for(&account)?;
        let frames = adapter
            .chat_stream(&account.snapshot(), &request, &client)
            .await?;
        let mut frames = frames;
        let mut events = Vec::new();
        use futures_util::StreamExt as _;
        while let Some(frame) = frames.next().await {
            match frame {
                Ok(frame) => events.extend(adapter.parse_event(&frame)),
                Err(error) => return Err(error),
            }
        }
        let completion =
            crate::gateway::protocol::encode::openai::fold(&events, "probe", &upstream_model, 0)?;
        Ok::<_, GatewayError>(completion)
    };

    /*
      端点 PING 与对话探活**并行**发起。

      为什么不等对话探完再 PING：串行会把两段耗时加在一起（对话 3s + PING 200ms），
      而且 PING 的价值正是"在模型还没回话时先告诉用户网络是通的"。
      它自带 5 秒超时，最坏情况也不会拖长整轮探活 —— 但为了不让它拖住结果，
      spawn 出去后台跑，取结果时再 join。

      目标 origin 由适配器给出（`endpoint_origin`）：那是渠道知识，
      路由层不该硬编码任何域名。返回 `None` 的渠道（假渠道）就不 PING。
    */
    let ping_handle = match (
        adapter.endpoint_origin(&account.snapshot()),
        state.client_for(&account),
    ) {
        (Some(origin), Ok(client)) => Some(tokio::spawn(async move {
            crate::gateway::transport::ping_origin(&client, &origin).await
        })),
        (Some(_), Err(_)) => None,
        (None, _) => None,
    };

    /*
      第一次失败时，若上游说的是"凭据不认"，就强制刷新一次再探。

      为什么探活也要做这一步：探活是**发现故障**的手段，如果它和普通请求一样
      只在"临期 2 分钟"才刷新，那么"被上游提前吊销但本地没过期"的号会每轮
      稳定报 401 —— 面板上那条红字看起来像永久的，其实账号一刷就能好。
      刷新失败才把 401 如实报出去（那时真的需要重新登录）。
    */
    let first = tokio::time::timeout(timeout, attempt()).await;
    let outcome = match first {
        Ok(Err(error)) if error.kind == ErrorKind::AuthExpired => {
            match refresh::refresh(state, account, true).await {
                Ok(_) => {
                    eprintln!(
                        "[gateway] 探活遇到 401，已强制刷新凭据并重试 {}",
                        account.key
                    );
                    tokio::time::timeout(timeout, attempt()).await
                }
                Err(refresh_error) => {
                    account.apply_error(&error, &state.pool.policy(), now_ms());
                    return Err(GatewayError::new(
                        error.kind,
                        error.scope,
                        error.status,
                        format!(
                            "{}；强制刷新凭据失败：{}",
                            error.message, refresh_error.message
                        ),
                        error.retryable,
                    ));
                }
            }
        }
        other => other,
    };

    match outcome {
        Err(_) => {
            let error = GatewayError::network("探针超时");
            account.apply_error(&error, &state.pool.policy(), now_ms());
            Err(error)
        }
        Ok(Err(error)) => {
            account.apply_error(&error, &state.pool.policy(), now_ms());
            Err(error)
        }
        Ok(Ok(completion)) => {
            // 成功即清错误状态（与调度路径一致）。
            account.on_success(&request.model);
            let message = &completion["choices"][0]["message"];
            let reply = message["content"]
                .as_str()
                .filter(|text| !text.is_empty())
                .or_else(|| message["reasoning_content"].as_str())
                .unwrap_or("")
                .to_string();
            let usage = &completion["usage"];
            Ok(ProbeOutcome {
                model: chosen,
                upstream_model,
                reply: clip(&reply, 80),
                latency_ms: started.elapsed().as_millis() as u64,
                /*
                  端点 PING 与对话**并行**跑完再取结果。

                  为什么不让它参与 `latency_ms`：那两个数回答的是不同问题 ——
                  「对话延迟」= 模型多久开始回话（含上游排队、推理），
                  「端点 PING」= 这台主机在网络层多久回包。混在一起
                  就再也分不清"是我网络慢"还是"上游模型慢"。

                  PING 失败不影响探活结论（`None`）：有的主机对 HEAD 直接 405，
                  那是端点策略，不是故障。
                */
                ping_latency_ms: match ping_handle {
                    Some(handle) => handle.await.ok().flatten(),
                    None => None,
                },
                prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
                completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
            })
        }
    }
}

/// 单独探一次端点 PING（对话失败时用：区分"上不去"和"上游报错"）。
///
/// 为什么失败路径要单独再探一次：`probe_account` 只在成功时带回 ping
/// （失败时整个 `ProbeOutcome` 都不存在），而"对话失败时网络到底通不通"
/// 恰恰是排障最想知道的那一条 —— 网络断了要修网络，网络通着就是上游的事。
///
/// 返回 `None` = 这个渠道不提供 PING，或请求本身失败。
pub async fn ping_endpoint(state: &GatewayState, account: &Arc<PoolAccount>) -> Option<u64> {
    let adapter = state.adapter_for(account)?;
    let origin = adapter.endpoint_origin(&account.snapshot())?;
    let client = state.client_for(account).ok()?;
    crate::gateway::transport::ping_origin(&client, &origin).await
}

/// 探针结果（面板展示）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeOutcome {
    pub model: String,
    pub upstream_model: String,
    pub reply: String,
    /// 对话延迟：从发请求到整条回复收完的端到端耗时。
    pub latency_ms: u64,
    /// 端点 PING：对该渠道 origin 的一次 HEAD 往返；测不到时为 None。
    pub ping_latency_ms: Option<u64>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(limit.saturating_sub(3)).collect();
    clipped.push_str("...");
    clipped
}

#[cfg(test)]
mod probe_tests {
    //! 真实账号探针（手动执行）。
    //!
    //! 参考项目从桌面端的 APPDATA 目录读 SQLite 与系统凭据库；服务端版本改成
    //! 读环境变量里的 `DATABASE_URL` 与 `RELAYAI_MASTER_KEY`，与生产同一条路径。
    //!
    //! 执行方式：
    //! ```text
    //! cargo test probe_real_account -- --ignored --nocapture
    //! ```
    //! 需要同时设置 DATABASE_URL 与 RELAYAI_MASTER_KEY，且库里有一个可用账号。

    use super::*;
    use crate::gateway::{config::Config, storage::Storage};

    #[tokio::test]
    #[ignore = "需要真实数据库与账号，手动执行"]
    async fn probe_real_account() {
        let data_source = std::env::var("DATABASE_URL").expect("需要 DATABASE_URL");
        let master_key = crate::secrets::load_master_key().expect("需要 RELAYAI_MASTER_KEY");

        let storage = Storage::open(&data_source, master_key).await.expect("库");
        let config = Config::default();
        let state = GatewayState::initialize(config, storage)
            .await
            .expect("状态");

        let Some(account) = state.pool.accounts().into_iter().next() else {
            panic!("号池为空：先通过管理台添加一个账号");
        };

        let outcome = probe_account(&state, &account, None, std::time::Duration::from_secs(45))
            .await
            .expect("探针通过");

        println!("=== 探针结果 ===");
        println!(
            "{}",
            serde_json::to_string_pretty(&outcome).unwrap_or_default()
        );
    }
}
