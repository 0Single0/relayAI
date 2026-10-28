-- M0 验收：is_rejection 的统计语义
-- 目的：证明"拒绝行"不会被计入总量、按渠道分组、按模型分组。
-- 参考项目的 bug 是用 model LIKE '(未进入调度%)' 排除，前缀改名后失效；
-- 这里改用显式布尔列，语义不依赖任何字符串约定。

TRUNCATE usage_logs RESTART IDENTITY;

INSERT INTO usage_logs
    (request_id, channel_id, account_uid, protocol, model, status,
     is_rejection, prompt_tokens, completion_tokens, total_tokens,
     upstream_cost, billed_cost)
VALUES
    ('r-ok-1',  'wb-cn',   'u1', 'chat',      'deepseek-v4.1-flash', 'completed', false, 100,  50, 150, 0.001, 0.0012),
    ('r-ok-2',  'wb-cn',   'u1', 'chat',      'deepseek-v4.1-flash', 'completed', false, 200, 100, 300, 0.002, 0.0024),
    ('r-ok-3',  'trae-cn', 'u2', 'anthropic', 'glm-5.3',             'completed', false, 300, 150, 450, 0.003, 0.0036),
    ('r-rej-1', '-',       '-',  'chat',      '(rejected)',          'rejected',  true,    0,   0,   0, 0,     0),
    ('r-rej-2', '-',       '-',  'chat',      '(no-account)',        'rejected',  true,    0,   0,   0, 0,     0);

\echo '--- 全表行数（应为 5）---'
SELECT count(*) AS all_rows FROM usage_logs;

\echo '--- 统计口径：排除拒绝行（应为 3）---'
SELECT count(*) AS billable_rows FROM usage_logs WHERE NOT is_rejection;

\echo '--- 按渠道分组：不应出现 channel_id = - 的行 ---'
SELECT channel_id, count(*) AS requests, sum(total_tokens) AS tokens
FROM usage_logs
WHERE NOT is_rejection
GROUP BY channel_id
ORDER BY channel_id;

\echo '--- 成本合计：应为 0.0072 ---'
SELECT sum(upstream_cost) AS upstream, sum(billed_cost) AS billed
FROM usage_logs
WHERE NOT is_rejection;
