-- 修正历史遗留的「0」设置值。
--
-- ---------------------------------------------------------------------------
-- 为什么需要这一步
-- ---------------------------------------------------------------------------
-- `settings` 表里的值是**覆盖**默认值的：只要存过一个 0，它就会一直盖住
-- Go 里的默认值 —— 哪怕后来把默认值改得更合理。
--
-- 实测到的问题：
--   · `probe.interval_minutes = 0` → 探活间隔非法，定时器按 5 分钟兜底，
--     但设置页显示 0，与"最短 5 分钟"的提示自相矛盾；
--   · `pricing.usd_to_cny = 0` → 汇率缺失，美元价折算成 0（模型全变免费）；
--   · `payload.retention_days = 0` → 留存 0 天，等于永远不存。
--
-- 这里只修**业务上不成立**的值（0 / 负数），不动用户设过的有效值 ——
-- "和默认值不同"不是修它的理由，那会把用户的刻意选择也改掉。
--
-- `probe.enabled = false` 是**合法选择**（有人就是不想烧额度），
-- 所以本迁移不碰它。默认值已改为"开启"，新库开箱即用；
-- 老库如果关着，保持关着 —— 那是用户的选择。

-- 探活间隔：0 或小于 5 都不成立。
UPDATE settings
   SET value = '5'
 WHERE key = 'probe.interval_minutes'
   AND value::text ~ '^-?[0-9]+$'
   AND value::text::numeric < 5;

-- 汇率：0 会让所有美元价折算成 0。
UPDATE settings
   SET value = '7.2'
 WHERE key = 'pricing.usd_to_cny'
   AND value::text ~ '^-?[0-9.]+$'
   AND value::text::numeric <= 0;

-- 留存天数：0 天等于不存。
UPDATE settings
   SET value = '7'
 WHERE key = 'payload.retention_days'
   AND value::text ~ '^-?[0-9]+$'
   AND value::text::numeric <= 0;

COMMENT ON TABLE settings IS
    '系统设置。值是默认值的**覆盖**：这里存了 0 就会盖住 Go 里的默认值，见迁移 017。';
