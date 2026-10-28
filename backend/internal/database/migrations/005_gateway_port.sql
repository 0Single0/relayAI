-- 网关默认端口从 8788 改为 18788。
--
-- 原因：8788 与 8790 在本机已被其他常驻服务占用（pythonw、参考项目），
-- 网关用这两个端口必然触发端口回退，回退后 GATEWAY_BASE_URL 与实际监听端口
-- 不一致，管理面就会打到别人的服务上。
--
-- 只改**仍然是旧默认值 8788** 的行：如果使用者已经手工把它改成别的端口，
-- 说明那是有意配置的，迁移不应该覆盖。
UPDATE settings
   SET value = '18788'::jsonb,
       updated_at = now()
 WHERE key = 'gateway.port'
   AND value = '8788'::jsonb;
