-- 上游地址与协议下沉到账号（渠道变成纯计价层）。
--
-- 为什么改：原来的设计里 `channels.options` 存 `base_url` / `protocol`，
-- 而 API Key 存在账号上 —— 一份凭据被拆到两张表，且**建渠道时填的地址
-- 与建账号时填的 Key 可能对不上**。更糟的是单独建一个「自定义渠道」
-- 除了填个地址之外没有任何作用，因为真正的凭据还不存在。
--
-- sub2api 的分层是：
--   · Account = 一份上游凭据（platform + type + credentials{api_key, base_url}）
--   · Channel = 计价与模型映射层（倍率、模型改名、限哪些模型）
-- 也就是说**渠道不持有凭据**。这里对齐这个分层。
--
-- `protocol` 也一起下沉：同一把 Key 可能只支持 chat 或只支持 responses，
-- 这属于凭据的属性，不是计费策略的属性。
ALTER TABLE accounts ADD COLUMN base_url TEXT NOT NULL DEFAULT '';
ALTER TABLE accounts ADD COLUMN protocol TEXT NOT NULL DEFAULT 'openai-chat';

COMMENT ON COLUMN accounts.base_url IS '上游基址；空 = 用适配器内置默认（WorkBuddy/Trae 一般留空）';
COMMENT ON COLUMN accounts.protocol IS '上游协议：openai-chat / openai-responses / anthropic';

-- 已有自定义渠道的地址搬到它名下的账号上，避免升级后这些账号突然没有地址。
UPDATE accounts a
   SET base_url = COALESCE(c.options ->> 'base_url', ''),
       protocol = COALESCE(c.options ->> 'protocol', 'openai-chat')
  FROM channels c
 WHERE c.id = a.channel_id
   AND c.adapter = 'custom'
   AND a.base_url = '';

-- 渠道侧不再需要这两个键；留着会让"改了渠道地址"这种无效操作看起来有效。
UPDATE channels
   SET options = options - 'base_url' - 'protocol' - 'require_api_key'
 WHERE adapter = 'custom';
