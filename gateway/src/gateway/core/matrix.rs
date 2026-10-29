//! 模型 × 渠道能力矩阵：路由的第一层过滤器。
//!
//! 「glm 只能走 workbuddy、ds 能走 workbuddy + qoder」这件事**不写死在代码里**：
//! 各渠道目录里有什么模型，矩阵里就有什么候选；渠道上新一次目录刷新即生效。
//! 人工只做覆盖（别名、优先级、上限、临时禁用）。

use std::collections::HashMap;

use crate::gateway::{
    channels::{ModelCapabilities, UpstreamModel},
    config::RouteOverride,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    /// 渠道实时目录。
    #[default]
    Live,
    /// 内置快照（实时拉取失败时的回落）。
    Builtin,
    /// 人工覆盖。
    Override,
}

/// 能力字段的来源：决定"这个 false 到底算不算数"。
///
/// 早先这里是一个布尔 `caps_known`，含义是"是否来自实时元数据"，
/// 但它被同时当成两件事用，于是两头都不对：
///
/// · **调度**上把快照条目判成"能力未知"→ 快照里明确写着的能力位完全不参与过滤，
///   等于那份对照表白维护；
/// · **界面**上把"我们自己维护的对照表"显示成「能力未知」→ 卡片上出现
///   「工具调用 视觉 推理」和「能力未知」并列，看起来像自相矛盾。
///
/// 拆成三档后两者各归各位：`Live` / `Snapshot` 都是"有依据"，可以参与过滤，
/// 只有 `None`（上游没声明、我们也没有对照记录）才乐观放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsSource {
    /// 上游实时目录里带的能力位。
    Live,
    /// 内置对照表（快照）里的能力位：可信，但可能比上游实际能力旧。
    Snapshot,
    /// 完全没有依据：不做能力过滤，交给上游报错。
    #[default]
    None,
}

impl CapsSource {
    /// 是否"有依据"——有依据才拿 `declared_* && !flag` 去剔除候选。
    pub fn is_declared(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelLimits {
    pub context: Option<u64>,
    pub max_output: Option<u64>,
}

/// 一条「对外模型 → 某渠道」的路由。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelRoute {
    pub public_model: String,
    pub display_name: Option<String>,
    pub channel_id: String,
    /// 上游真实模型名（可能与对外名不同）。
    pub upstream_model: String,
    pub priority: u32,
    pub weight: u32,
    pub enabled: bool,
    pub credits: Option<String>,
    pub limits: ModelLimits,
    pub capabilities: ModelCapabilities,
    /// 上下文档位 / 思考档位 / 采样参数（面板要能一眼看清这个模型怎么用）。
    pub context_windows: Vec<u64>,
    pub reasoning_efforts: Vec<String>,
    pub reasoning_fixed_effort: Option<String>,
    pub reasoning_default_effort: Option<String>,
    pub reasoning_can_disable: Option<bool>,
    pub always_reasoning: bool,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// 能力字段的**来源**：`live` / `snapshot` / `none`（见 [`CapsSource`]）。
    ///
    /// `none` 时不做能力过滤（乐观放行，让上游报错），避免"因为未知就一律拒绝"；
    /// `snapshot` 是我们自己维护的对照表，必须参与过滤 —— 否则那份表白维护。
    pub caps_source: CapsSource,
    pub source: RouteSource,
}

impl ModelRoute {
    pub fn is_free(&self) -> bool {
        crate::gateway::channels::credits_is_free(self.credits.as_deref())
    }
}

/// 请求对候选渠道的硬性要求。不满足的候选在本地就被剔除（不发上游、不耗额度）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirements {
    pub vision: bool,
    pub tools: bool,
    /// 输入 token 估算值。
    pub context: u64,
    pub max_output: Option<u64>,
}

impl Requirements {
    pub fn from_request(
        request: &crate::gateway::protocol::cir::CanonicalRequest,
        estimated_prompt_tokens: u64,
    ) -> Self {
        let vision = request.messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, crate::gateway::protocol::cir::Block::Image { .. }))
        });
        Self {
            vision,
            tools: !request.tools.is_empty(),
            context: estimated_prompt_tokens,
            max_output: request.max_tokens,
        }
    }
}

/// 一条渠道的目录快照。
pub struct ChannelCatalog {
    pub channel_id: String,
    pub priority: u32,
    pub weight: u32,
    pub models: Vec<UpstreamModel>,
    /// 目录是否来自实时拉取（决定 source 标记）。
    pub live: bool,
}

#[derive(Debug, Default)]
pub struct Matrix {
    routes: HashMap<String, Vec<ModelRoute>>,
    aliases: HashMap<String, String>,
    built_at_ms: u64,
}

impl Matrix {
    pub fn build(
        catalogs: &[ChannelCatalog],
        aliases: &HashMap<String, String>,
        overrides: &[RouteOverride],
    ) -> Self {
        let mut routes: HashMap<String, Vec<ModelRoute>> = HashMap::new();

        for catalog in catalogs {
            for model in &catalog.models {
                // 即使实时目录里出现了内部模型，快照回落时也不该把它当聊天模型上架。
                // **例外**：用户显式声明的模型（自定义供应商的清单）就是要用的名字 ——
                // 命名清洗是某个渠道家族的约定，不该把用户填的 `auto` 静默吞掉。
                if !model.user_declared && !is_public_model(&model.id) {
                    continue;
                }
                let source = if catalog.live && !model.from_snapshot {
                    RouteSource::Live
                } else {
                    RouteSource::Builtin
                };
                routes
                    .entry(model.id.clone())
                    .or_default()
                    .push(ModelRoute {
                        public_model: model.id.clone(),
                        display_name: model.display_name.clone(),
                        channel_id: catalog.channel_id.clone(),
                        upstream_model: model.id.clone(),
                        priority: catalog.priority,
                        weight: catalog.weight,
                        enabled: true,
                        credits: model.credits.clone(),
                        limits: ModelLimits {
                            context: model.context_length,
                            max_output: model.max_output_tokens,
                        },
                        capabilities: model.capabilities.clone(),
                        context_windows: model.context_windows.clone(),
                        reasoning_efforts: model.reasoning_efforts.clone(),
                        reasoning_fixed_effort: model.reasoning_fixed_effort.clone(),
                        reasoning_default_effort: model.reasoning_default_effort.clone(),
                        reasoning_can_disable: model.reasoning_can_disable,
                        always_reasoning: model.always_reasoning,
                        temperature: model.temperature,
                        top_p: model.top_p,
                        // 三档而不是布尔：快照的能力位是我们自己维护的对照表，
                        // 有依据、该参与过滤，只是可能比上游实际能力旧；
                        // 真正"什么都没说"的（实时目录没带能力位）才乐观放行。
                        caps_source: caps_source_of(model),
                        source,
                    });
            }
        }

        apply_overrides(&mut routes, overrides);

        Self {
            routes,
            aliases: aliases.clone(),
            built_at_ms: crate::gateway::timeutil::now_ms(),
        }
    }

    pub fn built_at_ms(&self) -> u64 {
        self.built_at_ms
    }

    /// 解析别名后取出该对外模型的候选渠道（已剔除禁用的）。
    pub fn routes(&self, public_model: &str) -> Vec<ModelRoute> {
        self.all_routes(public_model)
            .into_iter()
            .filter(|route| route.enabled)
            .collect()
    }

    /// Management needs supported routes even when publication policy disables them.
    pub fn all_routes(&self, public_model: &str) -> Vec<ModelRoute> {
        let canonical = self.canonical(public_model);
        self.routes
            .get(&canonical)
            .map(|routes| {
                routes
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn canonical<'a>(&'a self, public_model: &'a str) -> String {
        self.aliases
            .get(public_model)
            .cloned()
            .unwrap_or_else(|| public_model.to_string())
    }

    /// 对外模型清单（含别名），供 `/v1/models` 与面板使用。
    pub fn public_models(&self) -> Vec<String> {
        let mut models: Vec<String> = self.routes.keys().cloned().collect();
        models.sort();
        models
    }

    pub fn route_count(&self) -> usize {
        self.routes.values().map(Vec::len).sum()
    }

    pub fn sources(&self) -> (usize, usize) {
        let live = self
            .routes
            .values()
            .flatten()
            .filter(|route| route.source == RouteSource::Live)
            .count();
        let builtin = self
            .routes
            .values()
            .flatten()
            .filter(|route| route.source == RouteSource::Builtin)
            .count();
        (live, builtin)
    }

    /// 该候选是否满足请求的硬性要求；不满足时返回可读原因。
    pub fn reject_reason(route: &ModelRoute, requirements: &Requirements) -> Option<String> {
        // 与工具同理：`vision == false` 可能是"明确不支持"，也可能只是"没声明"。
        if requirements.vision
            && route.caps_source.is_declared()
            && route.capabilities.declared_vision
            && !route.capabilities.vision
        {
            return Some("该渠道不支持图片输入".to_string());
        }
        // 工具同理，而且这里**必须**看 `declared_tools`：
        // `tool_calls == false` 既可能是"明确不支持"，也可能只是"目录没提"。
        // 早先只看能力来源（由 vision/reasoning 推导），于是 Trae 这种
        // "有 vision、有 reasoning、但目录没写 tool_call"的渠道被判成
        // 不支持工具调用，带 tools 的请求在本地 400，根本发不出去。
        if requirements.tools
            && route.caps_source.is_declared()
            && route.capabilities.declared_tools
            && !route.capabilities.tool_calls
        {
            return Some("该渠道不支持工具调用".to_string());
        }
        if let Some(context) = route.limits.context {
            if context > 0 && requirements.context > context {
                return Some(format!("上下文超出该渠道上限（{context}）"));
            }
        }
        // 注意：**不**用 max_tokens 过滤候选。
        // 客户端常发一个远大于上限的值（整段上下文长度、1M 之类），
        // 上游对这种情况是静默钳制到真实上限，而不是报错；
        // 网关照做即可（见 dispatch 里的钳制），拒绝反而会把唯一候选剔掉。
        None
    }
}

/// 内部/虚拟模型不上架（与渠道目录的清洗规则一致）。
fn is_public_model(id: &str) -> bool {
    crate::gateway::channels::is_chat_model(id)
}

/// 这条目录条目的能力字段算不算"有依据"，以及依据来自哪。
///
/// 三档判据：
/// 1. 快照条目（`from_snapshot`）→ `Snapshot`：三个位来自我们自己的对照表，
///    可信但会过时，仍应参与过滤；
/// 2. 实时目录里带了能力位 → `Live`；
/// 3. 其余（实时目录只有 id / 长度，能力位一个都没有）→ `None`，乐观放行。
///
/// **不能把"知道上下文长度"算作证据** —— 大量 OpenAI 兼容端点
/// （中转站 / 本地推理）只返回 id 与 context_length。早先把长度也算进来，
/// 这类供应商全被判定成"已知不支持图片/工具"，带工具的请求在本地就被剔除，
/// 表现为"接进来了却用不了"。
fn caps_source_of(model: &UpstreamModel) -> CapsSource {
    if model.from_snapshot {
        return CapsSource::Snapshot;
    }
    let has_meta = model.capabilities.vision
        || model.capabilities.tool_calls
        || model.capabilities.reasoning
        // 明确声明过"不支持"同样是依据：declared_* 为真即说明上游表过态。
        || model.capabilities.declared_vision
        || model.capabilities.declared_tools;
    if has_meta {
        CapsSource::Live
    } else {
        CapsSource::None
    }
}

fn apply_overrides(routes: &mut HashMap<String, Vec<ModelRoute>>, overrides: &[RouteOverride]) {
    for entry in overrides {
        let canonical = entry.model.clone();
        let existing = routes
            .get_mut(&canonical)
            .and_then(|list| list.iter_mut().find(|route| route.channel_id == entry.channel_id));

        match existing {
            Some(route) => {
                if let Some(upstream) = &entry.upstream_model {
                    route.upstream_model = upstream.clone();
                }
                if let Some(priority) = entry.priority {
                    route.priority = priority;
                }
                if let Some(weight) = entry.weight {
                    route.weight = weight;
                }
                if let Some(enabled) = entry.enabled {
                    route.enabled = enabled;
                }
                route.source = RouteSource::Override;
            }
            None => {
                // 目录里没有这条（渠道不提供该模型）：只有显式给了 upstream_model 才补一条，
                // 否则视为"配置写错"而不是"悄悄多一个候选"。
                let Some(upstream) = entry.upstream_model.clone() else {
                    continue;
                };
                routes.entry(canonical.clone()).or_default().push(ModelRoute {
                    public_model: canonical.clone(),
                    display_name: None,
                    channel_id: entry.channel_id.clone(),
                    upstream_model: upstream,
                    priority: entry.priority.unwrap_or(100),
                    weight: entry.weight.unwrap_or(1),
                    enabled: entry.enabled.unwrap_or(true),
                    credits: None,
                    limits: ModelLimits::default(),
                    capabilities: ModelCapabilities::default(),
                    context_windows: Vec::new(),
                    reasoning_efforts: Vec::new(),
                    reasoning_fixed_effort: None,
                    reasoning_default_effort: None,
                    reasoning_can_disable: None,
                    always_reasoning: false,
                    temperature: None,
                    top_p: None,
                    // 人工覆盖补出来的路由没有任何能力信息 —— 真正意义上的未知。
                    caps_source: CapsSource::None,
                    source: RouteSource::Override,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(id: &str, credits: Option<&str>, context: Option<u64>) -> UpstreamModel {
        UpstreamModel {
            id: id.into(),
            credits: credits.map(str::to_string),
            context_length: context,
            capabilities: ModelCapabilities {
                vision: false,
                tool_calls: true,
                reasoning: true,
                declared_tools: true,
                declared_vision: true,
            },
            ..UpstreamModel::default()
        }
    }

    fn catalogs() -> Vec<ChannelCatalog> {
        vec![
            ChannelCatalog {
                channel_id: "wb-cn".into(),
                priority: 100,
                weight: 5,
                live: true,
                models: vec![
                    model("deepseek-v4.1-flash", Some("x0.00"), Some(1_000_000)),
                    model("glm-5.3", Some("x0.00"), None),
                ],
            },
            ChannelCatalog {
                channel_id: "qoder".into(),
                priority: 90,
                weight: 3,
                live: true,
                models: vec![model("deepseek-v4.1-flash", Some("x0.20"), Some(200_000))],
            },
        ]
    }

    #[test]
    fn a_model_present_in_two_channels_yields_two_candidates() {
        let matrix = Matrix::build(&catalogs(), &HashMap::new(), &[]);
        let routes = matrix.routes("deepseek-v4.1-flash");
        assert_eq!(routes.len(), 2, "ds 应该同时命中 wk 与 qoder");
        let channels: Vec<&str> = routes.iter().map(|route| route.channel_id.as_str()).collect();
        assert!(channels.contains(&"wb-cn") && channels.contains(&"qoder"));
    }

    #[test]
    fn a_model_present_in_one_channel_stays_single_candidate() {
        let matrix = Matrix::build(&catalogs(), &HashMap::new(), &[]);
        let routes = matrix.routes("glm-5.3");
        assert_eq!(routes.len(), 1, "glm 只有 wk 一个候选");
        assert_eq!(routes[0].channel_id, "wb-cn");
        // 不存在的模型没有候选（上层据此本地 400，而不是发上游）。
        assert!(matrix.routes("gpt-9").is_empty());
    }

    #[test]
    fn aliases_resolve_to_the_canonical_model() {
        let matrix = Matrix::build(
            &catalogs(),
            &[("ds-flash".to_string(), "deepseek-v4.1-flash".to_string())]
                .into_iter()
                .collect(),
            &[],
        );
        assert_eq!(matrix.routes("ds-flash").len(), 2);
        assert_eq!(matrix.canonical("ds-flash"), "deepseek-v4.1-flash");
    }

    #[test]
    fn requirements_filter_out_unsuitable_candidates() {
        let matrix = Matrix::build(&catalogs(), &HashMap::new(), &[]);
        let strict = ModelRoute {
            caps_source: CapsSource::Live,
            limits: ModelLimits {
                context: Some(1000),
                max_output: Some(100),
            },
            ..matrix.routes("glm-5.3")[0].clone()
        };
        let reason = Matrix::reject_reason(
            &strict,
            &Requirements {
                vision: true,
                tools: false,
                context: 0,
                max_output: None,
            },
        );
        assert!(reason.expect("不支持图片应被拒").contains("图片"));

        // 超输出上限**不**拒绝：上游会钳制，网关照做（见 router 的钳制用例）。
        assert!(Matrix::reject_reason(
            &strict,
            &Requirements {
                vision: false,
                tools: true,
                context: 0,
                max_output: Some(500),
            },
        )
        .is_none());

        let reason = Matrix::reject_reason(
            &strict,
            &Requirements {
                vision: false,
                tools: false,
                context: 5000,
                max_output: None,
            },
        );
        assert!(reason.expect("超上下文应被拒").contains("上下文"));

        assert!(Matrix::reject_reason(&strict, &Requirements::default()).is_none());
    }

    /// **回归**：目录没写 tool_call、但写了 vision/reasoning 的渠道，
    /// 不能被判成"不支持工具调用"。
    ///
    /// 这正是 Trae 的实况：它有 multimodal 与 reasoning，目录里却没有 tool_call 键。
    /// 早先能力来源由 vision/reasoning 推导为"已知"，而 `tool_calls` 缺省回落 false，
    /// 于是带 tools 的请求在本地就 400 了（`该渠道不支持工具调用`）。
    #[test]
    fn undeclared_tool_support_is_not_treated_as_unsupported() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "trae-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "deepseek-v4.1-flash".into(),
                capabilities: ModelCapabilities {
                    // 目录只声明了图片与思考，**没提工具**。
                    vision: true,
                    tool_calls: false,
                    reasoning: true,
                    declared_tools: false,
                    declared_vision: true,
                },
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        let route = &matrix.routes("deepseek-v4.1-flash")[0];
        assert_eq!(
            route.caps_source,
            CapsSource::Live,
            "有 vision/reasoning 就说明能力元数据可信"
        );
        let requirements = Requirements {
            vision: false,
            tools: true,
            context: 0,
            max_output: None,
        };
        assert!(
            Matrix::reject_reason(route, &requirements).is_none(),
            "没声明工具能力 ≠ 不支持工具，必须放行"
        );
    }

    /// 反过来：目录**明确**写了不支持工具时，仍然要过滤（别修过头）。
    #[test]
    fn explicitly_unsupported_tools_are_still_rejected() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "no-tools".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "m".into(),
                capabilities: ModelCapabilities {
                    vision: false,
                    tool_calls: false,
                    reasoning: true,
                    declared_tools: true,
                    declared_vision: true,
                },
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        let route = &matrix.routes("m")[0];
        let requirements = Requirements {
            vision: false,
            tools: true,
            context: 0,
            max_output: None,
        };
        let reason = Matrix::reject_reason(route, &requirements);
        assert!(
            reason.as_deref().is_some_and(|text| text.contains("工具")),
            "明确声明不支持时必须过滤：{reason:?}"
        );
    }
    #[test]
    fn unknown_capabilities_do_not_reject_requests() {
        // caps_source=none：不能因为"完全没数据"就拒绝带图片/工具的请求。
        let route = ModelRoute {
            public_model: "hy3".into(),
            channel_id: "wb-cn".into(),
            upstream_model: "hy3".into(),
            priority: 100,
            weight: 1,
            enabled: true,
            credits: None,
            limits: ModelLimits::default(),
            capabilities: ModelCapabilities::default(),
            caps_source: CapsSource::None,
            source: RouteSource::Builtin,
            ..ModelRoute::default()
        };
        let requirements = Requirements {
            vision: true,
            tools: true,
            context: 10,
            max_output: Some(10),
        };
        assert!(Matrix::reject_reason(&route, &requirements).is_none());
    }

    /// 快照条目的能力位**有依据**：写着不支持就应当过滤，而不是被当成"未知"放行。
    ///
    /// 这是拆 `caps_source` 的直接动机：早先快照一律 `known=false`，
    /// 于是内置对照表里明确标注的能力在调度里完全不起作用。
    #[test]
    fn snapshot_capabilities_are_enforced() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "wb-cn".into(),
            priority: 100,
            weight: 1,
            live: false,
            models: vec![UpstreamModel {
                id: "hy3-old".into(),
                capabilities: ModelCapabilities {
                    vision: false,
                    tool_calls: true,
                    reasoning: true,
                    declared_tools: true,
                    declared_vision: true,
                },
                from_snapshot: true,
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        let route = &matrix.routes("hy3-old")[0];
        assert_eq!(route.caps_source, CapsSource::Snapshot);

        let reason = Matrix::reject_reason(
            route,
            &Requirements {
                vision: true,
                tools: false,
                context: 0,
                max_output: None,
            },
        );
        assert!(
            reason.as_deref().is_some_and(|text| text.contains("图片")),
            "快照写着不支持图片，就该在本地剔除：{reason:?}"
        );
    }

    #[test]
    fn overrides_can_disable_prioritise_and_add_routes() {
        let overrides: Vec<RouteOverride> = serde_json::from_value(json!([
            { "model": "deepseek-v4.1-flash", "channel_id": "qoder", "enabled": false },
            { "model": "glm-5.3", "channel_id": "wb-cn", "priority": 7 },
            { "model": "qwen-max", "channel_id": "qoder", "upstream_model": "qwen-max-preview", "priority": 50 }
        ]))
        .expect("覆盖配置");

        let matrix = Matrix::build(&catalogs(), &HashMap::new(), &overrides);
        let ds = matrix.routes("deepseek-v4.1-flash");
        assert_eq!(ds.len(), 1, "被禁用的渠道候选要消失");
        assert_eq!(ds[0].channel_id, "wb-cn");
        assert_eq!(matrix.all_routes("deepseek-v4.1-flash").len(), 2,
            "management must retain supported models when publication is disabled");

        let glm = matrix.routes("glm-5.3");
        assert_eq!(glm[0].priority, 7);
        assert_eq!(glm[0].source, RouteSource::Override);

        let qwen = matrix.routes("qwen-max");
        assert_eq!(qwen.len(), 1);
        assert_eq!(qwen[0].upstream_model, "qwen-max-preview", "上游名可以不同");
    }

    #[test]
    fn override_without_upstream_model_cannot_invent_a_candidate() {
        let overrides: Vec<RouteOverride> = serde_json::from_value(json!([
            { "model": "not-in-catalog", "channel_id": "qoder" }
        ]))
        .expect("覆盖配置");
        let matrix = Matrix::build(&catalogs(), &HashMap::new(), &overrides);
        assert!(matrix.routes("not-in-catalog").is_empty());
    }

    /// 账号改名会自动补一条"对外名 → 本渠道"的路由覆盖。补出来的这条要能
    /// 真的被请求到，且上游名指向我们映射的那个值（这正是 Trae 的场景）。
    #[test]
    fn rename_backed_route_is_requestable_and_points_at_the_upstream_name() {
        // Trae 目录里只有官方拼写；我们想把对外的 deepseek-v4.1-flash 落到它上面。
        let catalogs = vec![ChannelCatalog {
            channel_id: "trae-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "DeepSeek-V4-Flash-Official".into(),
                ..UpstreamModel::default()
            }],
        }];
        let overrides: Vec<RouteOverride> = serde_json::from_value(json!([{
            "model": "deepseek-v4.1-flash",
            "channel_id": "trae-cn",
            "upstream_model": "DeepSeek-V4-Flash-Official",
            "enabled": true
        }]))
        .expect("覆盖配置");

        let matrix = Matrix::build(&catalogs, &HashMap::new(), &overrides);
        let routes = matrix.routes("deepseek-v4.1-flash");
        assert_eq!(routes.len(), 1, "补出来的路由必须能被请求到");
        assert_eq!(routes[0].channel_id, "trae-cn");
        assert_eq!(
            routes[0].upstream_model, "DeepSeek-V4-Flash-Official",
            "出站要用 Trae 的上游名"
        );
        assert_eq!(routes[0].source, RouteSource::Override);
        // 上游原名的路由仍然在（改名不该把原模型下架）。
        assert_eq!(matrix.routes("DeepSeek-V4-Flash-Official").len(), 1);
    }

    /// 上游名拼错（不在目录里）时不要凭空造路由 —— 后端用它做校验门。
    #[test]
    fn rename_to_a_name_outside_the_catalog_is_guarded_by_the_matrix() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "trae-cn".into(),
            priority: 100,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "DeepSeek-V4-Flash-Official".into(),
                ..UpstreamModel::default()
            }],
        }];
        // 校验门在命令层（直接问渠道要目录）；这里确认矩阵本身不会替我们"猜"。
        let overrides: Vec<RouteOverride> = serde_json::from_value(json!([{
            "model": "deepseek-v4.1-flash",
            "channel_id": "trae-cn"
        }]))
        .expect("覆盖配置");
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &overrides);
        assert!(
            matrix.routes("deepseek-v4.1-flash").is_empty(),
            "没给 upstream_model 的覆盖不能创造候选"
        );
    }
    #[test]
    fn internal_models_never_reach_the_matrix() {
        let catalogs = vec![ChannelCatalog {
            channel_id: "wb-cn".into(),
            priority: 1,
            weight: 1,
            live: true,
            models: vec![model("lite", None, None), model("auto", None, None)],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        assert_eq!(matrix.route_count(), 0);
    }

    #[test]
    fn user_declared_model_names_survive_the_family_cleanup() {
        // 自定义供应商里用户填的模型就叫 `auto`：命名清洗是渠道家族的约定，
        // 不该把用户显式声明的名字静默吞掉（否则"加了模型却一个都没有"）。
        let catalogs = vec![ChannelCatalog {
            channel_id: "my-provider".into(),
            priority: 1,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "auto".into(),
                user_declared: true,
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        assert_eq!(matrix.route_count(), 1);
        assert_eq!(matrix.routes("auto").len(), 1);
    }

    #[test]
    fn context_length_alone_does_not_prove_capabilities_are_known() {
        // 只给了上下文长度、没给能力位（大量 OpenAI 兼容端点的实况）：
        // 必须按"未知"处理并乐观放行，而不是判成"不支持工具/图片"。
        let catalogs = vec![ChannelCatalog {
            channel_id: "my-provider".into(),
            priority: 1,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "gpt-x".into(),
                context_length: Some(128_000),
                user_declared: true,
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        let route = &matrix.routes("gpt-x")[0];
        assert_eq!(
            route.caps_source,
            CapsSource::None,
            "只有长度不足以证明能力位可信"
        );
        assert!(Matrix::reject_reason(
            route,
            &Requirements {
                vision: true,
                tools: true,
                context: 1,
                max_output: None,
            },
        )
        .is_none());

        // 反过来：显式声明了能力位，就按它过滤（用户说了不支持图片就别发图片）。
        let catalogs = vec![ChannelCatalog {
            channel_id: "my-provider".into(),
            priority: 1,
            weight: 1,
            live: true,
            models: vec![UpstreamModel {
                id: "gpt-x".into(),
                capabilities: ModelCapabilities {
                    vision: false,
                    tool_calls: true,
                    reasoning: false,
                    declared_tools: true,
                    declared_vision: true,
                },
                user_declared: true,
                ..UpstreamModel::default()
            }],
        }];
        let matrix = Matrix::build(&catalogs, &HashMap::new(), &[]);
        let route = &matrix.routes("gpt-x")[0];
        assert_eq!(route.caps_source, CapsSource::Live);
        assert!(Matrix::reject_reason(
            route,
            &Requirements {
                vision: true,
                tools: false,
                context: 0,
                max_output: None,
            },
        )
        .is_some());
        assert!(Matrix::reject_reason(
            route,
            &Requirements {
                vision: false,
                tools: true,
                context: 0,
                max_output: None,
            },
        )
        .is_none());
    }
}
