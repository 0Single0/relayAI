-- 用量结算标记：这条用量是否已经扣过用户余额。
--
-- ---------------------------------------------------------------------------
-- 为什么需要它
-- ---------------------------------------------------------------------------
-- 网关算出了 `billed_cost` 并写进 `usage_logs`，但**从来没有从用户余额里扣过钱**。
-- 也就是说：用量、账单、价格全对，只有"扣钱"这一环是空的。
--
-- 这个字段让结算变成**幂等**的：
--   定时任务与手动触发可能同时跑，靠它保证同一条用量只扣一次。
--
-- 为什么不用"最后结算时间戳"之类的粗粒度方案：
-- 那种方案在并发下会漏扣或重扣 —— 而扣费重扣是不可接受的错误。
--
-- 默认 NULL = 未结算。历史数据也因此是未结算状态，下次结算会补扣；
-- 如果不想补扣历史，先执行：
--   UPDATE usage_logs SET settled_at = now() WHERE billed_cost > 0;

ALTER TABLE usage_logs ADD COLUMN IF NOT EXISTS settled_at TIMESTAMPTZ;

-- 部分索引：只索引未结算的行。
-- 结算任务每次都要找"还没结算的"，而绝大多数行已经结算过 ——
-- 全表索引在这里是浪费，部分索引小得多、也快得多。
CREATE INDEX IF NOT EXISTS usage_logs_unsettled_idx
    ON usage_logs (id)
    WHERE settled_at IS NULL AND billed_cost > 0;

COMMENT ON COLUMN usage_logs.settled_at IS
    '该用量的费用已扣到用户余额的时间；NULL = 未结算。用于幂等结算。';
