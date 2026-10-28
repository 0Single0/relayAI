-- M2 验收用：一个 fake 渠道 + 一个账号
-- fake 适配器不需要凭据，专门用来验证链路（不碰任何真实上游）。

INSERT INTO channels (id, name, adapter, enabled, priority, weight, options)
VALUES ('fake', '链路自测', 'fake', true, 100, 1, '{"model":"fake-model"}'::jsonb)
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    adapter = EXCLUDED.adapter,
    enabled = EXCLUDED.enabled,
    options = EXCLUDED.options;

INSERT INTO accounts (channel_id, uid, nickname, enabled, source)
VALUES ('fake', 'u1', '自测账号', true, 'manual')
ON CONFLICT (channel_id, uid) DO NOTHING;
