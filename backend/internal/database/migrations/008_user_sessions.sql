-- 会话表。
--
-- 单独一个迁移而不是塞进 007：007 已在部分环境执行过，
-- 改它不会重跑（`schema_migrations` 有记录），新表就永远建不出来。
CREATE TABLE user_sessions (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 只存 SHA-256。库被读走也拿不到可用令牌。
    token_hash  TEXT NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT user_sessions_token_key UNIQUE (token_hash)
);

CREATE INDEX user_sessions_user ON user_sessions (user_id);
CREATE INDEX user_sessions_expiry ON user_sessions (expires_at);

COMMENT ON COLUMN user_sessions.token_hash IS 'SHA-256(令牌明文)；明文只在签发时返回一次';
