-- 用户体系（注册 / 登录 / 余额 / 兑换码）。
--
-- 为什么现在才加：SPEC 最初是"单租户自用"，Key 就是全部身份。
-- 但用户要的是"用户能自己注册、用兑换码充余额"——那就必须有自己的用户表，
-- 并且 `usage_logs` 要能归到人头上（否则"这个月我花了多少"算不出来）。
--
-- 与 `api_keys` 的关系：一个用户可以有多把 Key。Key 仍然是转发时的凭证，
-- 用户是计费与额度的主体。**不合并**这两者：Key 可以随时吊销重发，
-- 而余额必须跟着人走。
--
-- 与 POST /v1/chat/completions 的鉴权关系保持不变：仍然只认 `sk-` Key。
-- 用户体系只作用于管理台与余额结算，不给转发路径增加第二套认证。

CREATE TABLE users (
    id                BIGSERIAL PRIMARY KEY,
    email             TEXT NOT NULL,
    -- 密码用 bcrypt 哈希。**绝不存明文**（SPEC I1 同理：凭据不进库、不进日志）。
    password_hash     TEXT NOT NULL,
    nickname          TEXT NOT NULL DEFAULT '',
    -- 余额。用 NUMERIC 而不是 float：钱不能用二进制浮点存，
    -- 0.1 + 0.2 != 0.3 在账单上是真实的对不上账。
    balance           NUMERIC(14, 6) NOT NULL DEFAULT 0,
    -- 累计充值额（只增不减，用于运营看"这个用户充了多少"）。
    total_recharged   NUMERIC(14, 6) NOT NULL DEFAULT 0,
    -- 角色：admin 能进管理台全功能；user 只能看自己的用量与 Key。
    role              TEXT NOT NULL DEFAULT 'user',
    enabled           BOOLEAN NOT NULL DEFAULT true,
    -- 注册来源：password / invite。留字段是为了将来接邀请制时能区分。
    source            TEXT NOT NULL DEFAULT 'password',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at     TIMESTAMPTZ,

    CONSTRAINT users_email_key UNIQUE (email),
    -- 邮箱不做到 RFC 5322 全量校验（那是个无底洞），只挡明显不是邮箱的输入。
    CONSTRAINT users_email_check CHECK (email LIKE '%_@_%'),
    CONSTRAINT users_role_check CHECK (role IN ('admin', 'user')),
    CONSTRAINT users_balance_check CHECK (balance >= 0)
);

CREATE INDEX users_created ON users (created_at DESC);

COMMENT ON COLUMN users.balance IS '可用余额；扣费时递减，不允许扣成负数';
COMMENT ON COLUMN users.role IS 'admin 可进管理台；user 只能看自己的数据';

-- ---------------------------------------------------------------------------
-- 兑换码
--
-- 一个码只能用一次。`used_by` 记录被谁用了，`used_at` 记录什么时候。
-- 为什么不做"多次使用"：单租户场景下兑换码就是充值凭证，
-- 一次性语义最不容易出事（重复兑换 = 白送钱）。
-- ---------------------------------------------------------------------------
CREATE TABLE redeem_codes (
    id           BIGSERIAL PRIMARY KEY,
    -- 码本身。用大写字母数字，去掉容易看错的 0/O/1/I（人工抄写场景）。
    code         TEXT NOT NULL,
    -- 面额（元）。
    amount       NUMERIC(14, 6) NOT NULL,
    -- 备注：哪个批次、什么活动发的。
    note         TEXT NOT NULL DEFAULT '',
    -- 有效期。NULL = 永不过期。
    expires_at   TIMESTAMPTZ,
    used_by      BIGINT REFERENCES users (id) ON DELETE SET NULL,
    used_at      TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT redeem_codes_code_key UNIQUE (code),
    CONSTRAINT redeem_codes_amount_check CHECK (amount > 0)
);

CREATE INDEX redeem_codes_unused ON redeem_codes (created_at DESC) WHERE used_at IS NULL;

COMMENT ON COLUMN redeem_codes.code IS '一次性兑换码；大写字母数字，已排除 0/O/1/I';
COMMENT ON COLUMN redeem_codes.used_by IS '使用者；NULL = 未使用';

-- ---------------------------------------------------------------------------
-- 余额流水
--
-- `users.balance` 是当前值的缓存，真正的账在这里。
-- 只写不改（与 usage_logs 同理）：对不上账时能一路倒推。
-- ---------------------------------------------------------------------------
CREATE TABLE balance_transactions (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 正数 = 充值，负数 = 消费。
    amount       NUMERIC(14, 6) NOT NULL,
    -- 变动后余额，便于对账时逐行核对（不用自己累加）。
    balance_after NUMERIC(14, 6) NOT NULL,
    kind         TEXT NOT NULL,
    -- 关联对象（兑换码 id / usage_log id）。
    ref_id       BIGINT,
    note         TEXT NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT balance_transactions_kind_check
        CHECK (kind IN ('redeem', 'usage', 'adjust'))
);

CREATE INDEX balance_transactions_user ON balance_transactions (user_id, created_at DESC);

COMMENT ON COLUMN balance_transactions.amount IS '正数充值、负数消费';
COMMENT ON COLUMN balance_transactions.balance_after IS '变动后余额，对账用';

-- ---------------------------------------------------------------------------
-- usage_logs 关联用户
--
-- 可以 NULL：网关转发时只有 Key，没有用户会话。
-- 记账时按 Key 反查其归属用户（见 Go 侧 recordUsageOwner）。
-- 老数据留 NULL，统计时归入"未归属"。
-- ---------------------------------------------------------------------------
ALTER TABLE usage_logs ADD COLUMN user_id BIGINT REFERENCES users (id) ON DELETE SET NULL;
CREATE INDEX usage_user_ts ON usage_logs (user_id, ts DESC);
COMMENT ON COLUMN usage_logs.user_id IS '归属用户；NULL = 未归属（老数据或免鉴权请求）';

-- api_keys 也挂到用户上：一把 Key 属于一个人。
ALTER TABLE api_keys ADD COLUMN user_id BIGINT REFERENCES users (id) ON DELETE CASCADE;
CREATE INDEX api_keys_user ON api_keys (user_id);
COMMENT ON COLUMN api_keys.user_id IS 'Key 的归属用户；NULL = 未分配（老数据）';
