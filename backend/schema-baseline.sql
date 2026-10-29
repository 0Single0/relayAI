--
-- PostgreSQL database dump
--

\restrict Y9xrWayw0oXxDlX596V5zTUfJUvnlLbmVw5SnOfOCbhnBxqdwcd82uJIWkX6nWV

-- Dumped from database version 18.2
-- Dumped by pg_dump version 18.2

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: accounts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.accounts (
    id bigint NOT NULL,
    channel_id text NOT NULL,
    uid text NOT NULL,
    nickname text DEFAULT ''::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    credential_cipher bytea,
    priority_override integer,
    weight_override integer,
    max_inflight integer,
    model_allow text[],
    model_rename jsonb DEFAULT '{}'::jsonb NOT NULL,
    source text DEFAULT 'manual'::text NOT NULL,
    added_at timestamp with time zone DEFAULT now() NOT NULL,
    base_url text DEFAULT ''::text NOT NULL,
    protocol text DEFAULT 'openai-chat'::text NOT NULL,
    platform text DEFAULT 'custom'::text NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    proxy_id bigint,
    CONSTRAINT accounts_max_inflight_check CHECK (((max_inflight IS NULL) OR (max_inflight > 0))),
    CONSTRAINT accounts_uid_check CHECK ((length(btrim(uid)) > 0))
);


--
-- Name: COLUMN accounts.credential_cipher; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.credential_cipher IS 'AES-256-GCM 密文；自定义渠道可为空';


--
-- Name: COLUMN accounts.model_allow; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.model_allow IS '该账号只服务哪些对外模型名；空 = 不限';


--
-- Name: COLUMN accounts.model_rename; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.model_rename IS '{对外模型名: 该账号的上游模型名}';


--
-- Name: COLUMN accounts.base_url; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.base_url IS '上游基址；空 = 用适配器内置默认（WorkBuddy/Trae 一般留空）';


--
-- Name: COLUMN accounts.protocol; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.protocol IS '上游协议：openai-chat / openai-responses / anthropic';


--
-- Name: COLUMN accounts.platform; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.platform IS '上游平台：workbuddy | trae | custom | fake';


--
-- Name: COLUMN accounts.options; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.options IS '适配器私有参数（如 workbuddy 的 realm / outbound_identity）';


--
-- Name: COLUMN accounts.proxy_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.accounts.proxy_id IS '出站代理；NULL = 直连。删除代理时置空（不级联删账号）。';


--
-- Name: accounts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.accounts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: accounts_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.accounts_id_seq OWNED BY public.accounts.id;


--
-- Name: affinity_bindings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.affinity_bindings (
    key text NOT NULL,
    channel_id text NOT NULL,
    uid text NOT NULL,
    upstream_model text NOT NULL,
    expires_at_ms bigint NOT NULL,
    hits integer DEFAULT 0 NOT NULL
);


--
-- Name: TABLE affinity_bindings; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.affinity_bindings IS '会话亲和绑定；仅用于重启后恢复缓存亲和，过期由定时任务清理';


--
-- Name: api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.api_keys (
    id bigint NOT NULL,
    name text NOT NULL,
    key_hash text NOT NULL,
    key_prefix text NOT NULL,
    channel_allow text[],
    model_allow text[],
    rpm_limit integer,
    daily_token_limit bigint,
    max_concurrency integer,
    enabled boolean DEFAULT true NOT NULL,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id bigint,
    key_plaintext text,
    CONSTRAINT api_keys_concurrency_check CHECK (((max_concurrency IS NULL) OR (max_concurrency > 0))),
    CONSTRAINT api_keys_daily_check CHECK (((daily_token_limit IS NULL) OR (daily_token_limit > 0))),
    CONSTRAINT api_keys_name_check CHECK ((length(btrim(name)) > 0)),
    CONSTRAINT api_keys_rpm_check CHECK (((rpm_limit IS NULL) OR (rpm_limit > 0)))
);


--
-- Name: COLUMN api_keys.key_hash; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.key_hash IS 'SHA-256(明文)，用于鉴权查找';


--
-- Name: COLUMN api_keys.key_prefix; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.key_prefix IS '展示用前缀，如 sk-abcd****';


--
-- Name: COLUMN api_keys.channel_allow; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.channel_allow IS '渠道实例 id 列表；空 = 不限';


--
-- Name: COLUMN api_keys.model_allow; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.model_allow IS '对外模型名列表；空 = 不限';


--
-- Name: COLUMN api_keys.user_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.user_id IS 'Key 的归属用户；NULL = 未分配（老数据）';


--
-- Name: COLUMN api_keys.key_plaintext; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.api_keys.key_plaintext IS '密钥明文，供界面反复查看与复制。上游凭据仍由主密钥加密，与此列无关。';


--
-- Name: api_keys_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.api_keys_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: api_keys_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.api_keys_id_seq OWNED BY public.api_keys.id;


--
-- Name: balance_transactions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.balance_transactions (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    amount numeric(14,6) NOT NULL,
    balance_after numeric(14,6) NOT NULL,
    kind text NOT NULL,
    ref_id bigint,
    note text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT balance_transactions_kind_check CHECK ((kind = ANY (ARRAY['redeem'::text, 'usage'::text, 'adjust'::text])))
);


--
-- Name: COLUMN balance_transactions.amount; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.balance_transactions.amount IS '正数充值、负数消费';


--
-- Name: COLUMN balance_transactions.balance_after; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.balance_transactions.balance_after IS '变动后余额，对账用';


--
-- Name: balance_transactions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.balance_transactions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: balance_transactions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.balance_transactions_id_seq OWNED BY public.balance_transactions.id;


--
-- Name: channel_caps; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.channel_caps (
    channel_id text NOT NULL,
    tasks boolean DEFAULT false NOT NULL,
    daily boolean DEFAULT false NOT NULL,
    checkin boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: TABLE channel_caps; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.channel_caps IS '渠道运营能力缓存；来源是网关的 ChannelAdapter::ops_caps()';


--
-- Name: COLUMN channel_caps.tasks; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channel_caps.tasks IS '有任务清单（可接取/上报/领奖），仅 workbuddy realm=cn';


--
-- Name: COLUMN channel_caps.daily; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channel_caps.daily IS '有每日动作清单，workbuddy 两个 realm 都有';


--
-- Name: COLUMN channel_caps.checkin; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channel_caps.checkin IS '有单次签到 hook；trae 只有这一项';


--
-- Name: channels; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.channels (
    id text NOT NULL,
    name text NOT NULL,
    adapter text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    priority integer DEFAULT 100 NOT NULL,
    weight integer DEFAULT 1 NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    channel_ratio numeric(10,4) DEFAULT 1.0 NOT NULL,
    concurrency_limit integer,
    rate_limit_qps integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    model_allow text[],
    probe_model text,
    CONSTRAINT channels_adapter_check CHECK ((adapter = ANY (ARRAY['workbuddy'::text, 'trae'::text, 'custom'::text, 'fake'::text]))),
    CONSTRAINT channels_priority_check CHECK (((priority >= 0) AND (priority <= 1000))),
    CONSTRAINT channels_ratio_check CHECK ((channel_ratio > (0)::numeric)),
    CONSTRAINT channels_weight_check CHECK ((weight > 0))
);


--
-- Name: COLUMN channels.id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channels.id IS '渠道实例 id，如 wb-cn / wb-intl / trae-cn';


--
-- Name: COLUMN channels.adapter; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channels.adapter IS '已废弃：平台已下沉到 accounts.platform。此列仅作历史记录，不要再用它做分支。';


--
-- Name: COLUMN channels.options; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channels.options IS '适配器私有参数，字段集由 adapter 决定（SPEC 5.1）';


--
-- Name: COLUMN channels.model_allow; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channels.model_allow IS '分组对外暴露的模型白名单；NULL/空 = 不限制。与账号级 model_allow（能力）不同，这是产品策略。';


--
-- Name: COLUMN channels.probe_model; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.channels.probe_model IS '探活使用的对外模型名；NULL/空 = 由网关按「免费优先」自动挑。';


--
-- Name: models; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.models (
    canonical_name text NOT NULL,
    display_name text DEFAULT ''::text NOT NULL,
    context_window integer,
    max_output integer,
    capabilities jsonb DEFAULT '{}'::jsonb NOT NULL,
    input_price numeric(14,8) DEFAULT 0 NOT NULL,
    output_price numeric(14,8) DEFAULT 0 NOT NULL,
    cache_read_price numeric(14,8) DEFAULT 0 NOT NULL,
    model_ratio numeric(10,4) DEFAULT 1.0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    CONSTRAINT models_price_check CHECK (((input_price >= (0)::numeric) AND (output_price >= (0)::numeric) AND (cache_read_price >= (0)::numeric))),
    CONSTRAINT models_ratio_check CHECK ((model_ratio > (0)::numeric))
);


--
-- Name: COLUMN models.capabilities; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.models.capabilities IS '{vision, tool_calls, reasoning}';


--
-- Name: COLUMN models.input_price; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.models.input_price IS '每 1M token 的价格';


--
-- Name: proxies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.proxies (
    id bigint NOT NULL,
    name text NOT NULL,
    protocol text DEFAULT 'socks5'::text NOT NULL,
    host text NOT NULL,
    port integer NOT NULL,
    username text DEFAULT ''::text NOT NULL,
    password_enc text DEFAULT ''::text NOT NULL,
    fallback text DEFAULT 'fail'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: COLUMN proxies.password_enc; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.proxies.password_enc IS '代理密码，主密钥加密。与账号凭据同一把密钥，明文永不落库。';


--
-- Name: proxies_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.proxies_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: proxies_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.proxies_id_seq OWNED BY public.proxies.id;


--
-- Name: redeem_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.redeem_codes (
    id bigint NOT NULL,
    code text NOT NULL,
    amount numeric(14,6) NOT NULL,
    note text DEFAULT ''::text NOT NULL,
    expires_at timestamp with time zone,
    used_by bigint,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT redeem_codes_amount_check CHECK ((amount > (0)::numeric))
);


--
-- Name: COLUMN redeem_codes.code; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.redeem_codes.code IS '一次性兑换码；大写字母数字，已排除 0/O/1/I';


--
-- Name: COLUMN redeem_codes.used_by; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.redeem_codes.used_by IS '使用者；NULL = 未使用';


--
-- Name: redeem_codes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.redeem_codes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: redeem_codes_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.redeem_codes_id_seq OWNED BY public.redeem_codes.id;


--
-- Name: schema_migrations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.schema_migrations (
    version text NOT NULL,
    applied_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.settings (
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: TABLE settings; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.settings IS '系统设置。值是默认值的**覆盖**：这里存了 0 就会盖住 Go 里的默认值，见迁移 017。';


--
-- Name: task_run_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.task_run_items (
    id bigint NOT NULL,
    run_id bigint NOT NULL,
    code text NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    status text NOT NULL,
    message text DEFAULT ''::text NOT NULL,
    earned integer DEFAULT 0 NOT NULL,
    finished_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT task_run_items_kind_check CHECK ((kind = ANY (ARRAY['daily'::text, 'task'::text, 'checkin'::text]))),
    CONSTRAINT task_run_items_status_check CHECK ((status = ANY (ARRAY['done'::text, 'skipped'::text, 'failed'::text, 'not_in_window'::text])))
);


--
-- Name: task_run_items_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.task_run_items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: task_run_items_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.task_run_items_id_seq OWNED BY public.task_run_items.id;


--
-- Name: task_runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.task_runs (
    id bigint NOT NULL,
    batch_id uuid,
    channel_id text NOT NULL,
    account_uid text NOT NULL,
    scope text NOT NULL,
    status text NOT NULL,
    trigger text NOT NULL,
    total integer DEFAULT 0 NOT NULL,
    done_count integer DEFAULT 0 NOT NULL,
    failed_count integer DEFAULT 0 NOT NULL,
    skipped_count integer DEFAULT 0 NOT NULL,
    earned integer DEFAULT 0 NOT NULL,
    error text,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    CONSTRAINT task_runs_scope_check CHECK ((scope = ANY (ARRAY['all'::text, 'daily'::text, 'tasks'::text, 'checkin'::text]))),
    CONSTRAINT task_runs_status_check CHECK ((status = ANY (ARRAY['running'::text, 'done'::text, 'partial'::text, 'failed'::text, 'cancelled'::text]))),
    CONSTRAINT task_runs_trigger_check CHECK ((trigger = ANY (ARRAY['manual'::text, 'schedule'::text])))
);


--
-- Name: task_runs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.task_runs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: task_runs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.task_runs_id_seq OWNED BY public.task_runs.id;


--
-- Name: tmp_probe; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tmp_probe (
    name text NOT NULL,
    v numeric(14,8)
);


--
-- Name: usage_logs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.usage_logs (
    id bigint NOT NULL,
    ts timestamp with time zone DEFAULT now() NOT NULL,
    request_id text NOT NULL,
    key_id bigint,
    channel_id text NOT NULL,
    account_uid text NOT NULL,
    protocol text NOT NULL,
    model text NOT NULL,
    upstream_model text,
    stream boolean DEFAULT false NOT NULL,
    status text NOT NULL,
    is_rejection boolean DEFAULT false NOT NULL,
    attempts integer DEFAULT 1 NOT NULL,
    prompt_tokens bigint DEFAULT 0 NOT NULL,
    completion_tokens bigint DEFAULT 0 NOT NULL,
    reasoning_tokens bigint DEFAULT 0 NOT NULL,
    cached_tokens bigint DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT 0 NOT NULL,
    usage_estimated boolean DEFAULT false NOT NULL,
    credit numeric(14,6),
    upstream_cost numeric(14,8) DEFAULT 0 NOT NULL,
    billed_cost numeric(14,8) DEFAULT 0 NOT NULL,
    ttft_ms integer,
    total_ms integer DEFAULT 0 NOT NULL,
    effort text,
    error_detail text,
    user_id bigint,
    settled_at timestamp with time zone,
    input_price numeric(14,8),
    output_price numeric(14,8),
    cache_read_price numeric(14,8),
    model_ratio numeric(10,6),
    channel_ratio numeric(10,6),
    CONSTRAINT usage_protocol_check CHECK ((protocol = ANY (ARRAY['chat'::text, 'responses'::text, 'anthropic'::text]))),
    CONSTRAINT usage_status_check CHECK ((status = ANY (ARRAY['completed'::text, 'failed'::text, 'client_aborted'::text, 'upstream_aborted'::text, 'rejected'::text])))
);


--
-- Name: COLUMN usage_logs.key_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.key_id IS '无外键：Key 可删，历史记录要留';


--
-- Name: COLUMN usage_logs.is_rejection; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.is_rejection IS '未进入调度的请求，统计时排除';


--
-- Name: COLUMN usage_logs.upstream_cost; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.upstream_cost IS '上游真实成本，与 billed_cost 分开存（SPEC I4）';


--
-- Name: COLUMN usage_logs.billed_cost; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.billed_cost IS '记账额度 = upstream_cost × model_ratio × channel_ratio';


--
-- Name: COLUMN usage_logs.user_id; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.user_id IS '归属用户；NULL = 未归属（老数据或免鉴权请求）';


--
-- Name: COLUMN usage_logs.settled_at; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.settled_at IS '该用量的费用已扣到用户余额的时间；NULL = 未结算。用于幂等结算。';


--
-- Name: COLUMN usage_logs.input_price; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.input_price IS '计费时的输入单价快照（元/百万 token，未乘倍率）；NULL = 该行早于快照功能';


--
-- Name: COLUMN usage_logs.output_price; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.output_price IS '计费时的输出单价快照（元/百万 token，未乘倍率）';


--
-- Name: COLUMN usage_logs.cache_read_price; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.cache_read_price IS '计费时的缓存读取单价快照（元/百万 token，未乘倍率）';


--
-- Name: COLUMN usage_logs.model_ratio; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.model_ratio IS '计费时的模型倍率快照；记账额度 = 上游成本 × model_ratio × channel_ratio';


--
-- Name: COLUMN usage_logs.channel_ratio; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.usage_logs.channel_ratio IS '计费时的分组倍率快照';


--
-- Name: usage_logs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.usage_logs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: usage_logs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.usage_logs_id_seq OWNED BY public.usage_logs.id;


--
-- Name: usage_payloads; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.usage_payloads (
    usage_id bigint NOT NULL,
    request jsonb DEFAULT '{}'::jsonb NOT NULL,
    response jsonb DEFAULT '{}'::jsonb NOT NULL,
    bytes integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: TABLE usage_payloads; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON TABLE public.usage_payloads IS '请求/响应内容留存，默认关闭（settings.payload.enabled）。隐私敏感，保留天数独立配置。';


--
-- Name: user_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_sessions (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: COLUMN user_sessions.token_hash; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.user_sessions.token_hash IS 'SHA-256(令牌明文)；明文只在签发时返回一次';


--
-- Name: user_sessions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.user_sessions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: user_sessions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.user_sessions_id_seq OWNED BY public.user_sessions.id;


--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id bigint NOT NULL,
    email text NOT NULL,
    password_hash text NOT NULL,
    nickname text DEFAULT ''::text NOT NULL,
    balance numeric(14,6) DEFAULT 0 NOT NULL,
    total_recharged numeric(14,6) DEFAULT 0 NOT NULL,
    role text DEFAULT 'user'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    source text DEFAULT 'password'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone,
    CONSTRAINT users_balance_check CHECK ((balance >= (0)::numeric)),
    CONSTRAINT users_email_check CHECK ((email ~~ '%_@_%'::text)),
    CONSTRAINT users_role_check CHECK ((role = ANY (ARRAY['admin'::text, 'user'::text])))
);


--
-- Name: COLUMN users.balance; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.balance IS '可用余额；扣费时递减，不允许扣成负数';


--
-- Name: COLUMN users.role; Type: COMMENT; Schema: public; Owner: -
--

COMMENT ON COLUMN public.users.role IS 'admin 可进管理台；user 只能看自己的数据';


--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--
-- Name: accounts id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts ALTER COLUMN id SET DEFAULT nextval('public.accounts_id_seq'::regclass);


--
-- Name: api_keys id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys ALTER COLUMN id SET DEFAULT nextval('public.api_keys_id_seq'::regclass);


--
-- Name: balance_transactions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.balance_transactions ALTER COLUMN id SET DEFAULT nextval('public.balance_transactions_id_seq'::regclass);


--
-- Name: proxies id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.proxies ALTER COLUMN id SET DEFAULT nextval('public.proxies_id_seq'::regclass);


--
-- Name: redeem_codes id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redeem_codes ALTER COLUMN id SET DEFAULT nextval('public.redeem_codes_id_seq'::regclass);


--
-- Name: task_run_items id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_run_items ALTER COLUMN id SET DEFAULT nextval('public.task_run_items_id_seq'::regclass);


--
-- Name: task_runs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_runs ALTER COLUMN id SET DEFAULT nextval('public.task_runs_id_seq'::regclass);


--
-- Name: usage_logs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.usage_logs ALTER COLUMN id SET DEFAULT nextval('public.usage_logs_id_seq'::regclass);


--
-- Name: user_sessions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions ALTER COLUMN id SET DEFAULT nextval('public.user_sessions_id_seq'::regclass);


--
-- Name: users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--
-- Name: accounts accounts_channel_uid_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_channel_uid_key UNIQUE (channel_id, uid);


--
-- Name: accounts accounts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);


--
-- Name: affinity_bindings affinity_bindings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.affinity_bindings
    ADD CONSTRAINT affinity_bindings_pkey PRIMARY KEY (key);


--
-- Name: api_keys api_keys_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_hash_key UNIQUE (key_hash);


--
-- Name: api_keys api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);


--
-- Name: balance_transactions balance_transactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.balance_transactions
    ADD CONSTRAINT balance_transactions_pkey PRIMARY KEY (id);


--
-- Name: channel_caps channel_caps_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.channel_caps
    ADD CONSTRAINT channel_caps_pkey PRIMARY KEY (channel_id);


--
-- Name: channels channels_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.channels
    ADD CONSTRAINT channels_pkey PRIMARY KEY (id);


--
-- Name: models models_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_pkey PRIMARY KEY (canonical_name);


--
-- Name: proxies proxies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.proxies
    ADD CONSTRAINT proxies_pkey PRIMARY KEY (id);


--
-- Name: redeem_codes redeem_codes_code_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_code_key UNIQUE (code);


--
-- Name: redeem_codes redeem_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_pkey PRIMARY KEY (id);


--
-- Name: schema_migrations schema_migrations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.schema_migrations
    ADD CONSTRAINT schema_migrations_pkey PRIMARY KEY (version);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (key);


--
-- Name: task_run_items task_run_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_run_items
    ADD CONSTRAINT task_run_items_pkey PRIMARY KEY (id);


--
-- Name: task_runs task_runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_runs
    ADD CONSTRAINT task_runs_pkey PRIMARY KEY (id);


--
-- Name: tmp_probe tmp_probe_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tmp_probe
    ADD CONSTRAINT tmp_probe_pkey PRIMARY KEY (name);


--
-- Name: usage_logs usage_logs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.usage_logs
    ADD CONSTRAINT usage_logs_pkey PRIMARY KEY (id);


--
-- Name: usage_payloads usage_payloads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.usage_payloads
    ADD CONSTRAINT usage_payloads_pkey PRIMARY KEY (usage_id);


--
-- Name: user_sessions user_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);


--
-- Name: user_sessions user_sessions_token_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_token_key UNIQUE (token_hash);


--
-- Name: users users_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);


--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--
-- Name: accounts_by_channel; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX accounts_by_channel ON public.accounts USING btree (channel_id, enabled);


--
-- Name: accounts_by_platform; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX accounts_by_platform ON public.accounts USING btree (platform);


--
-- Name: accounts_by_proxy; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX accounts_by_proxy ON public.accounts USING btree (proxy_id);


--
-- Name: affinity_bindings_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX affinity_bindings_expires ON public.affinity_bindings USING btree (expires_at_ms);


--
-- Name: api_keys_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX api_keys_user ON public.api_keys USING btree (user_id);


--
-- Name: balance_transactions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX balance_transactions_user ON public.balance_transactions USING btree (user_id, created_at DESC);


--
-- Name: proxies_by_endpoint; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX proxies_by_endpoint ON public.proxies USING btree (protocol, host, port);


--
-- Name: redeem_codes_unused; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX redeem_codes_unused ON public.redeem_codes USING btree (created_at DESC) WHERE (used_at IS NULL);


--
-- Name: task_run_items_run; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX task_run_items_run ON public.task_run_items USING btree (run_id, id);


--
-- Name: task_runs_account; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX task_runs_account ON public.task_runs USING btree (channel_id, account_uid, started_at DESC);


--
-- Name: task_runs_running; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX task_runs_running ON public.task_runs USING btree (status) WHERE (status = 'running'::text);


--
-- Name: task_runs_started; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX task_runs_started ON public.task_runs USING btree (started_at DESC);


--
-- Name: usage_account_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_account_ts ON public.usage_logs USING btree (account_uid, ts DESC);


--
-- Name: usage_channel_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_channel_ts ON public.usage_logs USING btree (channel_id, ts DESC);


--
-- Name: usage_key_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_key_ts ON public.usage_logs USING btree (key_id, ts DESC);


--
-- Name: usage_logs_unsettled_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_logs_unsettled_idx ON public.usage_logs USING btree (id) WHERE ((settled_at IS NULL) AND (billed_cost > (0)::numeric));


--
-- Name: usage_model_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_model_ts ON public.usage_logs USING btree (model, ts DESC);


--
-- Name: usage_payloads_by_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_payloads_by_created ON public.usage_payloads USING btree (created_at);


--
-- Name: usage_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_ts ON public.usage_logs USING btree (ts DESC);


--
-- Name: usage_user_ts; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX usage_user_ts ON public.usage_logs USING btree (user_id, ts DESC);


--
-- Name: user_sessions_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_sessions_expiry ON public.user_sessions USING btree (expires_at);


--
-- Name: user_sessions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_sessions_user ON public.user_sessions USING btree (user_id);


--
-- Name: users_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_created ON public.users USING btree (created_at DESC);


--
-- Name: accounts accounts_channel_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;


--
-- Name: accounts accounts_proxy_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES public.proxies(id) ON DELETE SET NULL;


--
-- Name: api_keys api_keys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: balance_transactions balance_transactions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.balance_transactions
    ADD CONSTRAINT balance_transactions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- Name: channel_caps channel_caps_channel_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.channel_caps
    ADD CONSTRAINT channel_caps_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.channels(id) ON DELETE CASCADE;


--
-- Name: redeem_codes redeem_codes_used_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_used_by_fkey FOREIGN KEY (used_by) REFERENCES public.users(id) ON DELETE SET NULL;


--
-- Name: task_run_items task_run_items_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_run_items
    ADD CONSTRAINT task_run_items_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.task_runs(id) ON DELETE CASCADE;


--
-- Name: usage_logs usage_logs_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.usage_logs
    ADD CONSTRAINT usage_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;


--
-- Name: usage_payloads usage_payloads_usage_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.usage_payloads
    ADD CONSTRAINT usage_payloads_usage_id_fkey FOREIGN KEY (usage_id) REFERENCES public.usage_logs(id) ON DELETE CASCADE;


--
-- Name: user_sessions user_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--

\unrestrict Y9xrWayw0oXxDlX596V5zTUfJUvnlLbmVw5SnOfOCbhnBxqdwcd82uJIWkX6nWV

