-- 用量记录的价格快照：把"当时按什么算的"存下来。
--
-- ---------------------------------------------------------------------------
-- 为什么必须存快照
-- ---------------------------------------------------------------------------
-- 原先 `usage_logs` 只有 `upstream_cost` / `billed_cost` 两个最终金额。
-- 价格一改（运营调价、拉 OpenRouter、改倍率），历史记录就无法还原计算过程 ——
-- 用户看到「$0.00275」想问"怎么算出来的"，谁也答不上来。
--
-- 更要紧的是**对账**：月底用户拿着自己的账单来问，如果没有当时的单价，
-- 只能用今天的价去算昨天的账，算出来必然对不上。
--
-- ---------------------------------------------------------------------------
-- 为什么这些列允许为 NULL
-- ---------------------------------------------------------------------------
-- 历史行（本迁移之前的）没有快照，只能为 NULL。
-- 界面据此显示"这条记录没有价格快照"，而不是编一个数字出来。
--
-- 倍率同理：`channel_ratio` 当时可能是 1.0，也可能没配过。

ALTER TABLE usage_logs
    -- 单价快照（元 / 百万 token，**未乘任何倍率**）
    ADD COLUMN IF NOT EXISTS input_price      NUMERIC(14, 8),
    ADD COLUMN IF NOT EXISTS output_price     NUMERIC(14, 8),
    ADD COLUMN IF NOT EXISTS cache_read_price NUMERIC(14, 8),
    -- 倍率快照：记账额度 = 上游成本 × model_ratio × channel_ratio
    ADD COLUMN IF NOT EXISTS model_ratio   NUMERIC(10, 6),
    ADD COLUMN IF NOT EXISTS channel_ratio NUMERIC(10, 6);

COMMENT ON COLUMN usage_logs.input_price IS
    '计费时的输入单价快照（元/百万 token，未乘倍率）；NULL = 该行早于快照功能';
COMMENT ON COLUMN usage_logs.output_price IS
    '计费时的输出单价快照（元/百万 token，未乘倍率）';
COMMENT ON COLUMN usage_logs.cache_read_price IS
    '计费时的缓存读取单价快照（元/百万 token，未乘倍率）';
COMMENT ON COLUMN usage_logs.model_ratio IS
    '计费时的模型倍率快照；记账额度 = 上游成本 × model_ratio × channel_ratio';
COMMENT ON COLUMN usage_logs.channel_ratio IS
    '计费时的分组倍率快照';
