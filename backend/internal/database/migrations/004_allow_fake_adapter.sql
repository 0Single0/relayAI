-- 允许 fake 适配器（链路自测用）。
--
-- 背景：001 的 CHECK 只列了三个真实适配器，结果连"验证网关是否通"都做不到 ——
-- 那只能靠真实账号试错，而真实账号会消耗额度、还可能触发风控。
--
-- fake 渠道不发任何上游请求，只验证「鉴权 → 选号 → 编码 → SSE」这条链路。

ALTER TABLE channels DROP CONSTRAINT IF EXISTS channels_adapter_check;

ALTER TABLE channels
    ADD CONSTRAINT channels_adapter_check
    CHECK (adapter IN ('workbuddy', 'trae', 'custom', 'fake'));

COMMENT ON COLUMN channels.adapter IS
    '适配器类型：workbuddy / trae / custom（真实上游），fake（链路自测）';
