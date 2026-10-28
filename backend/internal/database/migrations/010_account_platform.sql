-- 平台（适配器）下沉到账号；渠道变成纯粹的计价分组。
--
-- ---------------------------------------------------------------------------
-- 问题
-- ---------------------------------------------------------------------------
-- 原来是 `channels.adapter = workbuddy | trae | custom`。于是：
--   · 想加一个 WorkBuddy 账号，必须**先**建一个「WorkBuddy 类型」的渠道；
--   · 渠道列表上写的是"WorkBuddy / Trae / 自定义"，读起来像"渠道就是平台"；
--   · 同一个渠道下面不能混不同平台的账号，而现实中"低价通道"这种分组
--     本来就可能包含多家上游。
--
-- ---------------------------------------------------------------------------
-- 目标（对齐 sub2api 的分层）
-- ---------------------------------------------------------------------------
--   · **Account** = 一份上游凭据：platform + 凭据 + 地址/协议 + 少数适配器参数。
--     "我要加一个 WorkBuddy 号" → 直接在账号页选平台，不需要先建渠道。
--   · **Channel** = 计价分组：倍率、优先级、权重、并发/QPS 上限。
--     它不关心账号是什么平台，只关心"这些账号按什么价卖"。
--
-- 归属关系仍是「账号 → 一个渠道」（用户的说法："账号自己划分到某一个渠道里"）。
--
-- ---------------------------------------------------------------------------
-- 迁移策略
-- ---------------------------------------------------------------------------
-- 已有数据里，渠道的 adapter 就是它名下所有账号的平台 —— 直接搬过去，
-- 不会丢信息。渠道那一列**保留但不读**：删列会让回滚变难，而它反正不再参与
-- 任何判断，留着当历史记录。
ALTER TABLE accounts ADD COLUMN platform TEXT NOT NULL DEFAULT 'custom';
ALTER TABLE accounts ADD COLUMN options JSONB NOT NULL DEFAULT '{}'::jsonb;

COMMENT ON COLUMN accounts.platform IS '上游平台：workbuddy | trae | custom | fake';
COMMENT ON COLUMN accounts.options IS '适配器私有参数（如 workbuddy 的 realm / outbound_identity）';

-- 把渠道上的 adapter 搬到账号上。只搬"账号还没被显式设过平台"的情况
-- （默认值 'custom' 视为没设过，因为它是 ADD COLUMN 时填的）。
UPDATE accounts a
   SET platform = c.adapter
  FROM channels c
 WHERE c.id = a.channel_id
   AND a.platform = 'custom'
   AND c.adapter <> 'custom';

-- workbuddy 的 realm / outbound_identity 也跟着搬到账号上：
-- 它们描述"这个号是哪个区的、用什么出站身份"，属于凭据属性。
UPDATE accounts a
   SET options = c.options
  FROM channels c
 WHERE c.id = a.channel_id
   AND c.adapter = 'workbuddy'
   AND a.options = '{}'::jsonb
   AND c.options <> '{}'::jsonb;

CREATE INDEX accounts_by_platform ON accounts (platform);

-- 渠道不再需要适配器类型。保留列但把语义写清楚，避免后来的人又按它分支。
COMMENT ON COLUMN channels.adapter IS
    '已废弃：平台已下沉到 accounts.platform。此列仅作历史记录，不要再用它做分支。';
