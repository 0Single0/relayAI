-- 渠道的运营能力探测结果（SPEC 11.1）
--
-- 为什么落库而不是每次问网关：
--   ops_caps() 是纯函数、不发网络请求，但它是适配器知识。Go 侧要渲染「任务」菜单
--   的二级项，每次进页面都跨服务问一遍不划算，且网关重启期间菜单会空掉。
--   缓存到库里，网关不可用时菜单仍然稳定。
--
-- 什么时候刷新：渠道增删改后、网关启动后、手动点「刷新能力」。

CREATE TABLE channel_caps (
    channel_id TEXT PRIMARY KEY REFERENCES channels (id) ON DELETE CASCADE,
    tasks      BOOLEAN NOT NULL DEFAULT false,
    daily      BOOLEAN NOT NULL DEFAULT false,
    checkin    BOOLEAN NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE channel_caps IS
    '渠道运营能力缓存；来源是网关的 ChannelAdapter::ops_caps()';
COMMENT ON COLUMN channel_caps.tasks IS
    '有任务清单（可接取/上报/领奖），仅 workbuddy realm=cn';
COMMENT ON COLUMN channel_caps.daily IS
    '有每日动作清单，workbuddy 两个 realm 都有';
COMMENT ON COLUMN channel_caps.checkin IS
    '有单次签到 hook；trae 只有这一项';
