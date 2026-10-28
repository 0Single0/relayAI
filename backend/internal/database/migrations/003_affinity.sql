-- 会话亲和绑定（网关内部表）
--
-- SPEC I5 说运行态不落库，但亲和是个例外：进程重启后全部换号等于把上游的
-- 前缀缓存全丢掉，代价太大。所以落一张**小表**，只存绑定关系，不做统计查询。
--
-- 这条与 SPEC 的偏差记在这里：SPEC 第 2 节说"亲和绑定在 Rust 内存"，
-- 实际实现改为"内存为主 + PG 落盘恢复"。理由是重启成本，不是设计疏忽。
--
-- 用 IF NOT EXISTS：这份迁移曾经被手工执行过（排障时直接跑了 SQL），
-- 而迁移运行器只认 schema_migrations 里的记录。重复执行必须无害。

CREATE TABLE IF NOT EXISTS affinity_bindings (
    key            TEXT PRIMARY KEY,
    channel_id     TEXT NOT NULL,
    uid            TEXT NOT NULL,
    upstream_model TEXT NOT NULL,
    expires_at_ms  BIGINT NOT NULL,
    hits           INT NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS affinity_bindings_expires ON affinity_bindings (expires_at_ms);

COMMENT ON TABLE affinity_bindings IS
    '会话亲和绑定；仅用于重启后恢复缓存亲和，过期由定时任务清理';
