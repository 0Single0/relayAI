//! 计价。
//!
//! 三件事必须分清：
//!   · **上游成本**（`upstream_cost`）= token × 模型单价。这是"我花了多少"；
//!   · **模型倍率**（`model_ratio`）—— 同一个模型在不同渠道的加价系数；
//!   · **渠道倍率**（`channel_ratio`）—— 整条渠道的加价系数。
//!
//! 记账额度（`billed_cost`）= 上游成本 × 模型倍率 × 渠道倍率。
//!
//! 为什么必须分开记而不是只记最终价：毛利 = 记账 - 成本。
//! 只记一个数就没法回答"这个渠道到底赚不赚"。
//!
//! **价格从哪来**：网关不查 `models` 表（它的存储层只读自己需要的列）。
//! 管理面通过 `/internal/prices` 推一份「模型 → 单价」过来，网关缓存在内存里。
//! 好处是计价不额外打库；代价是改了价格要重新推一次 —— 管理面改完会调
//! `notifyReload`，走的就是这条路径。

use std::collections::HashMap;

/// 一个模型的价格（元 / 百万 token）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

/// 一次请求的 token 用量（计价只需要这三个数）。
#[derive(Debug, Clone, Copy, Default)]
pub struct BillableTokens {
    pub prompt: u64,
    pub completion: u64,
    pub cached: u64,
}

/// 计价结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct Billing {
    /// 上游成本（元）。
    pub upstream_cost: f64,
    /// 记账额度（元）。
    pub billed_cost: f64,
    /// 计费用的单价与倍率快照。
    ///
    /// `None` 表示找不到价格 —— 那种行的成本是 0，也不该假装有单价。
    pub snapshot: Option<crate::gateway::storage::PriceSnapshot>,
}

/// 一份「模型 → 单价」映射。
#[derive(Debug, Default)]
pub struct PriceBook {
    prices: HashMap<String, ModelPrice>,
}

impl PriceBook {
    pub fn new(prices: HashMap<String, ModelPrice>) -> Self {
        Self {
            prices: prices
                .into_iter()
                .map(|(name, price)| (name.to_lowercase(), price))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    /// 按模型名找价格。
    ///
    /// 只做忽略大小写的精确匹配，与管理面的内置价格填充一致。
    /// 日期版本、服务档位和路由变体可能有不同价格，必须单独定价；
    /// 不能让未定价型号在记账时又继承相似型号的价格。
    pub fn lookup(&self, model: &str) -> Option<ModelPrice> {
        let name = model.trim().to_lowercase();
        if name.is_empty() {
            return None;
        }

        self.prices.get(&name).copied()
    }
}

/// 算一次请求的成本与记账额度。
///
/// **缓存读取单独计价**：它通常只有输入价的 1/10，而长对话里缓存能占输入的
/// 80% 以上 —— 合成一个"输入价"会把成本算高好几倍。
/// 所以从 `prompt` 里**扣掉** `cached`，剩下的按输入价算。
///
/// 找不到价格时返回 0 并**不报错**：计价缺失不该让请求失败
/// （用户要的是回复，不是账单）。但这种行会留下 `upstream_cost = 0`，
/// 运维页可以据此发现"有模型没配价"。
pub fn compute(
    price: Option<ModelPrice>,
    tokens: BillableTokens,
    model_ratio: f64,
    channel_ratio: f64,
) -> Billing {
    let Some(price) = price else {
        return Billing::default();
    };

    // 1e6：价格是"每百万 token"。
    let cached = tokens.cached.min(tokens.prompt);
    let fresh_input = tokens.prompt.saturating_sub(cached);

    let upstream = (fresh_input as f64 * price.input
        + cached as f64 * if price.cache_read > 0.0 {
            price.cache_read
        } else {
            // 没有缓存价时按输入价算（保守：宁可算高，不要漏账）。
            price.input
        }
        + tokens.completion as f64 * price.output)
        / 1_000_000.0;

    // 倍率必须为正：库里的 0 会被当成"免费"，与"没配"混淆。
    let model_ratio = if model_ratio > 0.0 { model_ratio } else { 1.0 };
    let channel_ratio = if channel_ratio > 0.0 { channel_ratio } else { 1.0 };

    Billing {
        upstream_cost: upstream,
        billed_cost: upstream * model_ratio * channel_ratio,
        // 快照记录**实际参与计算**的三个单价与两个倍率。
        // 缓存价为 0 时上面按输入价算，快照里也如实记输入价 ——
        // 否则事后按快照复算会得到和账单不同的数。
        snapshot: Some(crate::gateway::storage::PriceSnapshot {
            input: price.input,
            output: price.output,
            cache_read: if price.cache_read > 0.0 {
                price.cache_read
            } else {
                price.input
            },
            model_ratio,
            channel_ratio,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price_book() -> PriceBook {
        let mut map = HashMap::new();
        map.insert(
            "gpt-4o".to_string(),
            ModelPrice { input: 18.0, output: 72.0, cache_read: 9.0 },
        );
        map.insert(
            "gpt-4o-mini".to_string(),
            ModelPrice { input: 1.0, output: 4.0, cache_read: 0.5 },
        );
        PriceBook::new(map)
    }

    #[test]
    fn prices_require_an_explicit_model_id() {
        let book = price_book();
        let found = book.lookup(" GPT-4O-MINI ").expect("应当命中");
        assert!((found.input - 1.0).abs() < 1e-9, "命中了错误的模型：{found:?}");

        let plain = book.lookup("gpt-4o").expect("应当命中");
        assert!((plain.input - 18.0).abs() < 1e-9);
        // The 2024-05-13 version has a different official price from gpt-4o.
        for model in ["gpt-4o-2024-05-13", "gpt-4o-mini-2099-01-01", "gpt-4o-unpublished"] {
            assert!(book.lookup(model).is_none(), "must not infer a price for {model}");
        }

        let mut prices = HashMap::new();
        prices.insert("gpt-4o".into(), ModelPrice { input: 2.5, ..ModelPrice::default() });
        prices.insert("gpt-4o-2024-05-13".into(), ModelPrice { input: 5.0, ..ModelPrice::default() });
        let book = PriceBook::new(prices);
        assert_eq!(book.lookup("gpt-4o-2024-05-13").unwrap().input, 5.0);
    }

    #[test]
    fn cached_tokens_are_billed_separately() {
        // 100 万输入里 80 万是缓存命中：成本必须显著低于全按输入价算。
        let price = Some(ModelPrice { input: 18.0, output: 72.0, cache_read: 1.8 });
        let billing = compute(
            price,
            BillableTokens { prompt: 1_000_000, completion: 0, cached: 800_000 },
            1.0,
            1.0,
        );

        // 20 万 × 18/1e6 + 80 万 × 1.8/1e6 = 3.6 + 1.44 = 5.04
        assert!(
            (billing.upstream_cost - 5.04).abs() < 1e-6,
            "实际 {}",
            billing.upstream_cost
        );
    }

    #[test]
    fn ratios_multiply_billed_cost() {
        let price = Some(ModelPrice { input: 10.0, output: 0.0, cache_read: 0.0 });
        let billing = compute(
            price,
            BillableTokens { prompt: 1_000_000, completion: 0, cached: 0 },
            2.0,
            0.5,
        );

        assert!((billing.upstream_cost - 10.0).abs() < 1e-6);
        // 10 × 2 × 0.5 = 10
        assert!((billing.billed_cost - 10.0).abs() < 1e-6);
    }

    #[test]
    fn missing_price_yields_zero_without_panicking() {
        let billing = compute(
            None,
            BillableTokens { prompt: 1000, completion: 500, cached: 0 },
            1.0,
            1.0,
        );
        assert_eq!(billing.upstream_cost, 0.0);
        assert_eq!(billing.billed_cost, 0.0);
    }

    #[test]
    fn missing_cache_price_falls_back_to_input_price() {
        // 没有缓存价时按输入价算：宁可算高，也不要漏账。
        let price = Some(ModelPrice { input: 10.0, output: 0.0, cache_read: 0.0 });
        let billing = compute(
            price,
            BillableTokens { prompt: 1_000_000, completion: 0, cached: 1_000_000 },
            1.0,
            1.0,
        );
        assert!((billing.upstream_cost - 10.0).abs() < 1e-6);
    }

    #[test]
    fn zero_ratio_is_treated_as_one() {
        // 倍率 0 在库里表示"没配"，不能当成免费。
        let price = Some(ModelPrice { input: 10.0, output: 0.0, cache_read: 0.0 });
        let billing = compute(
            price,
            BillableTokens { prompt: 1_000_000, completion: 0, cached: 0 },
            0.0,
            0.0,
        );
        assert!((billing.billed_cost - 10.0).abs() < 1e-6, "实际 {}", billing.billed_cost);
    }
}
