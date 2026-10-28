-- relayAI 初始化迁移
-- 对应 docs/SPEC.md 第 5 节「数据模型」。
--
-- 约定：
--   1. 本文件只在空库上执行一次，由 Go 侧迁移运行器在事务中执行并记录版本。
--   2. 时间列一律用 timestamptz，不存 epoch 毫秒（见 SPEC 10.3）。
--   3. 运行态（冷却、在飞并发、亲和绑定）不落库（SPEC I5）。
--   4. 凭据只存 AES-256-GCM 密文（SPEC I1）。

-- ---------------------------------------------------------------------------
-- 渠道
-- ---------------------------------------------------------------------------
CREATE TABLE channels (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    adapter           TEXT NOT NULL,
    enabled           BOOLEAN NOT NULL DEFAULT true,
    priority          INT NOT NULL DEFAULT 100,
    weight            INT NOT NULL DEFAULT 1,
    options           JSONB NOT NULL DEFAULT '{}'::jsonb,
    channel_ratio     NUMERIC(10, 4) NOT NULL DEFAULT 1.0,
    concurrency_limit INT,
    rate_limit_qps    INT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- fake 是链路自测用的假渠道（不需要凭据、不碰上游），
    -- 保留它是为了让"网关是否通"能被验证，而不是靠真实账号试错。
    CONSTRAINT channels_adapter_check
        CHECK (adapter IN ('workbuddy', 'trae', 'custom', 'fake')),
    CONSTRAINT channels_priority_check CHECK (priority >= 0 AND priority <= 1000),
    CONSTRAINT channels_weight_check CHECK (weight > 0),
    CONSTRAINT channels_ratio_check CHECK (channel_ratio > 0)
);

COMMENT ON COLUMN channels.id IS '渠道实例 id，如 wb-cn / wb-intl / trae-cn';
COMMENT ON COLUMN channels.options IS '适配器私有参数，字段集由 adapter 决定（SPEC 5.1）';

-- ---------------------------------------------------------------------------
-- 账号
--
-- 注意：没有 status / cooldown_until 列。冷却与可用性是运行态，只在网关内存里
-- 存在，重启丢失是可接受的（SPEC I5）。账号"已删除"就是物理删除。
-- ---------------------------------------------------------------------------
CREATE TABLE accounts (
    id                BIGSERIAL PRIMARY KEY,
    channel_id        TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    uid               TEXT NOT NULL,
    nickname          TEXT NOT NULL DEFAULT '',
    enabled           BOOLEAN NOT NULL DEFAULT true,
    credential_cipher BYTEA,
    priority_override INT,
    weight_override   INT,
    max_inflight      INT,
    model_allow       TEXT[],
    model_rename      JSONB NOT NULL DEFAULT '{}'::jsonb,
    source            TEXT NOT NULL DEFAULT 'manual',
    added_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT accounts_uid_check CHECK (length(btrim(uid)) > 0),
    CONSTRAINT accounts_max_inflight_check
        CHECK (max_inflight IS NULL OR max_inflight > 0),
    CONSTRAINT accounts_channel_uid_key UNIQUE (channel_id, uid)
);

CREATE INDEX accounts_by_channel ON accounts (channel_id, enabled);

COMMENT ON COLUMN accounts.credential_cipher IS 'AES-256-GCM 密文；自定义渠道可为空';
COMMENT ON COLUMN accounts.model_allow IS '该账号只服务哪些对外模型名；空 = 不限';
COMMENT ON COLUMN accounts.model_rename IS '{对外模型名: 该账号的上游模型名}';

-- ---------------------------------------------------------------------------
-- 模型目录与价格
--
-- 不手工维护全量清单：渠道目录实时拉取后与本表左连接，有则用本表的价格与
-- 能力，无则落一条 enabled=false 的占位并在界面上提示"未定价"（SPEC 5.3）。
-- ---------------------------------------------------------------------------
CREATE TABLE models (
    canonical_name   TEXT PRIMARY KEY,
    display_name     TEXT NOT NULL DEFAULT '',
    context_window   INT,
    max_output       INT,
    capabilities     JSONB NOT NULL DEFAULT '{}'::jsonb,
    input_price      NUMERIC(14, 8) NOT NULL DEFAULT 0,
    output_price     NUMERIC(14, 8) NOT NULL DEFAULT 0,
    cache_read_price NUMERIC(14, 8) NOT NULL DEFAULT 0,
    model_ratio      NUMERIC(10, 4) NOT NULL DEFAULT 1.0,
    enabled          BOOLEAN NOT NULL DEFAULT true,

    CONSTRAINT models_price_check CHECK (
        input_price >= 0 AND output_price >= 0 AND cache_read_price >= 0
    ),
    CONSTRAINT models_ratio_check CHECK (model_ratio > 0)
);

COMMENT ON COLUMN models.input_price IS '每 1M token 的价格';
COMMENT ON COLUMN models.capabilities IS '{vision, tool_calls, reasoning}';

-- ---------------------------------------------------------------------------
-- API Key
--
-- 明文永不落库（SPEC I2）：只存 SHA-256 与展示用前缀。
-- 两个 allowlist 都是"空 = 不限"，旧 Key 迁移后行为保持不变（SPEC 12.4）。
-- ---------------------------------------------------------------------------
CREATE TABLE api_keys (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL,
    key_hash          TEXT NOT NULL,
    key_prefix        TEXT NOT NULL,
    channel_allow     TEXT[],
    model_allow       TEXT[],
    rpm_limit         INT,
    daily_token_limit BIGINT,
    max_concurrency   INT,
    enabled           BOOLEAN NOT NULL DEFAULT true,
    expires_at        TIMESTAMPTZ,
    last_used_at      TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT api_keys_hash_key UNIQUE (key_hash),
    CONSTRAINT api_keys_name_check CHECK (length(btrim(name)) > 0),
    CONSTRAINT api_keys_rpm_check CHECK (rpm_limit IS NULL OR rpm_limit > 0),
    CONSTRAINT api_keys_daily_check
        CHECK (daily_token_limit IS NULL OR daily_token_limit > 0),
    CONSTRAINT api_keys_concurrency_check
        CHECK (max_concurrency IS NULL OR max_concurrency > 0)
);

COMMENT ON COLUMN api_keys.key_hash IS 'SHA-256(明文)，用于鉴权查找';
COMMENT ON COLUMN api_keys.key_prefix IS '展示用前缀，如 sk-abcd****';
COMMENT ON COLUMN api_keys.channel_allow IS '渠道实例 id 列表；空 = 不限';
COMMENT ON COLUMN api_keys.model_allow IS '对外模型名列表；空 = 不限';

-- ---------------------------------------------------------------------------
-- 用量
--
-- 一旦写入不可修改（SPEC I3）。所有统计都从本表派生，不另存汇总表。
--
-- is_rejection 是显式布尔列：参考项目用 model LIKE '(未进入调度%)' 做排除，
-- 但那个前缀后来改成了 '(上游失败 …)'，导致排除条件失效、拒绝请求被算进
-- 总量（SPEC 5.5）。不再用字符串前缀表达语义。
-- ---------------------------------------------------------------------------
CREATE TABLE usage_logs (
    id                BIGSERIAL PRIMARY KEY,
    ts                TIMESTAMPTZ NOT NULL DEFAULT now(),
    request_id        TEXT NOT NULL,
    key_id            BIGINT,
    channel_id        TEXT NOT NULL,
    account_uid       TEXT NOT NULL,
    protocol          TEXT NOT NULL,
    model             TEXT NOT NULL,
    upstream_model    TEXT,
    stream            BOOLEAN NOT NULL DEFAULT false,
    status            TEXT NOT NULL,
    is_rejection      BOOLEAN NOT NULL DEFAULT false,
    attempts          INT NOT NULL DEFAULT 1,
    prompt_tokens     BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    reasoning_tokens  BIGINT NOT NULL DEFAULT 0,
    cached_tokens     BIGINT NOT NULL DEFAULT 0,
    total_tokens      BIGINT NOT NULL DEFAULT 0,
    usage_estimated   BOOLEAN NOT NULL DEFAULT false,
    credit            NUMERIC(14, 6),
    upstream_cost     NUMERIC(14, 8) NOT NULL DEFAULT 0,
    billed_cost       NUMERIC(14, 8) NOT NULL DEFAULT 0,
    ttft_ms           INT,
    total_ms          INT NOT NULL DEFAULT 0,
    effort            TEXT,
    error_detail      TEXT,

    CONSTRAINT usage_status_check CHECK (
        status IN ('completed', 'failed', 'client_aborted', 'upstream_aborted', 'rejected')
    ),
    CONSTRAINT usage_protocol_check CHECK (
        protocol IN ('chat', 'responses', 'anthropic')
    )
);

COMMENT ON COLUMN usage_logs.key_id IS '无外键：Key 可删，历史记录要留';
COMMENT ON COLUMN usage_logs.is_rejection IS '未进入调度的请求，统计时排除';
COMMENT ON COLUMN usage_logs.upstream_cost IS '上游真实成本，与 billed_cost 分开存（SPEC I4）';
COMMENT ON COLUMN usage_logs.billed_cost IS '记账额度 = upstream_cost × model_ratio × channel_ratio';

CREATE INDEX usage_ts ON usage_logs (ts DESC);
CREATE INDEX usage_channel_ts ON usage_logs (channel_id, ts DESC);
CREATE INDEX usage_model_ts ON usage_logs (model, ts DESC);
CREATE INDEX usage_key_ts ON usage_logs (key_id, ts DESC);
CREATE INDEX usage_account_ts ON usage_logs (account_uid, ts DESC);

-- ---------------------------------------------------------------------------
-- 任务执行记录
--
-- 参考项目的 task_runs 是内存 HashMap 且从不清理（泄漏），也没有历史。
-- 这里改为 PG，内存只保留"运行中"的取消标志（SPEC 11.7）。
-- ---------------------------------------------------------------------------
CREATE TABLE task_runs (
    id            BIGSERIAL PRIMARY KEY,
    batch_id      UUID,
    channel_id    TEXT NOT NULL,
    account_uid   TEXT NOT NULL,
    scope         TEXT NOT NULL,
    status        TEXT NOT NULL,
    trigger       TEXT NOT NULL,
    total         INT NOT NULL DEFAULT 0,
    done_count    INT NOT NULL DEFAULT 0,
    failed_count  INT NOT NULL DEFAULT 0,
    skipped_count INT NOT NULL DEFAULT 0,
    earned        INT NOT NULL DEFAULT 0,
    error         TEXT,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at   TIMESTAMPTZ,

    CONSTRAINT task_runs_scope_check
        CHECK (scope IN ('all', 'daily', 'tasks', 'checkin')),
    CONSTRAINT task_runs_status_check
        CHECK (status IN ('running', 'done', 'partial', 'failed', 'cancelled')),
    CONSTRAINT task_runs_trigger_check
        CHECK (trigger IN ('manual', 'schedule'))
);

CREATE INDEX task_runs_started ON task_runs (started_at DESC);
CREATE INDEX task_runs_account ON task_runs (channel_id, account_uid, started_at DESC);
CREATE INDEX task_runs_running ON task_runs (status) WHERE status = 'running';

CREATE TABLE task_run_items (
    id          BIGSERIAL PRIMARY KEY,
    run_id      BIGINT NOT NULL REFERENCES task_runs (id) ON DELETE CASCADE,
    code        TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    status      TEXT NOT NULL,
    message     TEXT NOT NULL DEFAULT '',
    earned      INT NOT NULL DEFAULT 0,
    finished_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT task_run_items_kind_check
        CHECK (kind IN ('daily', 'task', 'checkin')),
    CONSTRAINT task_run_items_status_check
        CHECK (status IN ('done', 'skipped', 'failed', 'not_in_window'))
);

CREATE INDEX task_run_items_run ON task_run_items (run_id, id);

-- ---------------------------------------------------------------------------
-- 设置
-- ---------------------------------------------------------------------------
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 必须有默认值的键（SPEC 5.7）。
INSERT INTO settings (key, value) VALUES
    ('task.retention_days',        '90'::jsonb),
    ('task.schedule_enabled',      'true'::jsonb),
    ('task.schedule_hours',        '[9, 21]'::jsonb),
    ('task.per_account_gap_ms',    '800'::jsonb),
    ('task.daily_gap_ms',          '1200'::jsonb),
    ('task.report_gap_ms',         '1100'::jsonb),
    ('gateway.port',               '18788'::jsonb),
    ('gateway.bind',               '"127.0.0.1"'::jsonb),
    ('log.retention_days',         '30'::jsonb),
    ('notify.webhook_url',         '""'::jsonb)
ON CONFLICT (key) DO NOTHING;
