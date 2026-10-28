-- 出站代理（对齐 sub2api 的 proxies）。
--
-- ---------------------------------------------------------------------------
-- 为什么需要
-- ---------------------------------------------------------------------------
-- 上游按出口 IP 限流与封禁。所有账号共用服务器那一个 IP 时，
-- 几十个账号的流量会叠在一起，很容易被判定成异常流量 ——
-- 这是当前部署里最大的稳定性风险，且它表现为"上游偶发 429/403"，
-- 排查时很容易误以为是凭据或倍率问题。
--
-- 每个账号可以绑一个代理：不同账号走不同出口 IP，把风险摊开。
--
-- ---------------------------------------------------------------------------
-- 字段口径（照抄 sub2api）
-- ---------------------------------------------------------------------------
-- `protocol` 只支持 socks5 / http / https：reqwest 只认这三种。
-- 用户名密码分开存（而不是塞进 URL）：URL 里的密码要做百分号编码，
-- 一旦密码含 `@` 或 `:`，手拼 URL 就会拼错，而错误信息只说"连接失败"。

CREATE TABLE IF NOT EXISTS proxies (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL,
    protocol    TEXT NOT NULL DEFAULT 'socks5',
    host        TEXT NOT NULL,
    port        INT  NOT NULL,
    username    TEXT NOT NULL DEFAULT '',
    -- 密码加密存储（与账号凭据同一把主密钥），明文永不落库、不返回前端。
    password_enc TEXT NOT NULL DEFAULT '',
    -- 回退策略：代理不可用时怎么办。
    --   'fail'   = 直接失败（默认，最安全：不泄露真实 IP）
    --   'direct' = 直连（可用性优先，但会暴露服务器 IP）
    fallback    TEXT NOT NULL DEFAULT 'fail',
    enabled     BOOLEAN NOT NULL DEFAULT true,
    note        TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 同一 host:port 不重复建：重复条目只会让人不知道账号到底走了哪个。
CREATE UNIQUE INDEX IF NOT EXISTS proxies_by_endpoint
    ON proxies (protocol, host, port);

-- 账号绑定代理。NULL = 直连（沿用网关自身出口）。
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS proxy_id BIGINT
    REFERENCES proxies (id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS accounts_by_proxy ON accounts (proxy_id);

COMMENT ON COLUMN accounts.proxy_id IS
    '出站代理；NULL = 直连。删除代理时置空（不级联删账号）。';
COMMENT ON COLUMN proxies.password_enc IS
    '代理密码，主密钥加密。与账号凭据同一把密钥，明文永不落库。';
