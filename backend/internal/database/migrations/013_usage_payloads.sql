-- 请求内容留存（默认关闭）。
--
-- ---------------------------------------------------------------------------
-- 为什么需要
-- ---------------------------------------------------------------------------
-- 现在 `usage_logs` 只有元数据（模型、token、耗时），看不到"用户到底发了什么、
-- 模型回了什么"。出问题时无法复现：只能猜"是不是 prompt 太长""是不是模型没按
-- 格式回"。打开这个功能后可以逐条回放。
--
-- ---------------------------------------------------------------------------
-- 为什么默认关闭（重要）
-- ---------------------------------------------------------------------------
-- 这是**隐私敏感**功能：留存的是用户的完整对话内容。
--   · 默认关闭，必须显式打开；
--   · 打开时界面上要写明"会保存对话内容"；
--   · 保留天数独立于用量日志（内容比元数据敏感得多，通常更短）。
--
-- 存的是**脱敏后**的 JSON：`messages` / `response` / `tools` 三个字段。
-- 不存原始 HTTP 请求体（里面可能有客户端自己的 header、认证信息）。

CREATE TABLE IF NOT EXISTS usage_payloads (
    -- 与 usage_logs 一对一；删除日志时一起清掉。
    usage_id   BIGINT PRIMARY KEY REFERENCES usage_logs (id) ON DELETE CASCADE,
    -- 请求内容（脱敏后的 messages / system / tools）。
    request    JSONB NOT NULL DEFAULT '{}',
    -- 响应内容（最终文本 + 工具调用）。
    response   JSONB NOT NULL DEFAULT '{}',
    -- 字节数：用于在界面上提示"这条很大"，也便于估算占用。
    bytes      INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 按创建时间清理：清理任务的 WHERE 条件走这个索引。
CREATE INDEX IF NOT EXISTS usage_payloads_by_created ON usage_payloads (created_at);

COMMENT ON TABLE usage_payloads IS
    '请求/响应内容留存，默认关闭（settings.payload.enabled）。隐私敏感，保留天数独立配置。';
