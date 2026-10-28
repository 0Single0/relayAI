-- 任务模块验证用：一个 workbuddy 国内版渠道（不带真实账号）
--
-- 目的：验证「任务」菜单的二级项能出现、渠道任务页能渲染。
-- 不带账号时不会打上游，`/tasks/channels` 仍应列出该渠道（capabilities 有值）。

INSERT INTO channels (id, name, adapter, enabled, priority, weight, options)
VALUES (
    'wb-cn',
    'WorkBuddy 国内版',
    'workbuddy',
    true,
    100,
    5,
    '{"realm": "cn", "outbound_identity": "desktop"}'::jsonb
)
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    adapter = EXCLUDED.adapter,
    options = EXCLUDED.options;

-- 能力按适配器写入（与网关 ops_caps() 一致）。
INSERT INTO channel_caps (channel_id, tasks, daily, checkin)
VALUES ('wb-cn', true, true, true)
ON CONFLICT (channel_id) DO UPDATE SET
    tasks = EXCLUDED.tasks,
    daily = EXCLUDED.daily,
    checkin = EXCLUDED.checkin,
    updated_at = now();

-- Trae：只有签到（验证"只有签到"的退化形态）。
INSERT INTO channels (id, name, adapter, enabled, priority, weight, options)
VALUES (
    'trae-cn',
    'Trae 国内版',
    'trae',
    true,
    90,
    5,
    '{"base_url": "https://trae-api-cn.mchost.guru"}'::jsonb
)
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    adapter = EXCLUDED.adapter,
    options = EXCLUDED.options;

INSERT INTO channel_caps (channel_id, tasks, daily, checkin)
VALUES ('trae-cn', false, false, true)
ON CONFLICT (channel_id) DO UPDATE SET
    tasks = EXCLUDED.tasks,
    daily = EXCLUDED.daily,
    checkin = EXCLUDED.checkin,
    updated_at = now();
