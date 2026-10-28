-- 告警 webhook 地址（SPEC 9.13「通知」/ M8「掉号 1 分钟内收到通知」）。
--
-- 为什么要单独一个迁移而不是只改 001_init.sql：
-- 001 已经在所有环境执行过了（`schema_migrations` 里有记录），
-- 改它不会重跑，于是老库永远缺这一个键 —— 网关读不到，
-- 「通知」设置项就成了摆设。新键必须走增量迁移。
--
-- 默认空串 = **不发通知**。不默认给一个外部地址：单租户自用，
-- 用户没配就不该有任何出站请求。
INSERT INTO settings (key, value) VALUES
    ('notify.webhook_url', '""'::jsonb)
ON CONFLICT (key) DO NOTHING;
