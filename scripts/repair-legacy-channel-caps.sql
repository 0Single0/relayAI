-- Repair only missing capability records for the two imported WorkBuddy groups.
-- Existing accounts, credentials, group settings and capability rows are preserved.
BEGIN;

INSERT INTO channel_caps (channel_id, tasks, daily, checkin)
SELECT c.id, c.options->>'realm' = 'cn', true, true
FROM channels c
WHERE c.id IN ('wb-cn', 'wb-intl')
  AND c.adapter = 'workbuddy'
  AND c.options->>'realm' IN ('cn', 'intl')
  AND EXISTS (SELECT 1 FROM accounts a WHERE a.channel_id = c.id AND a.platform = 'workbuddy')
ON CONFLICT (channel_id) DO NOTHING;

SELECT c.id, c.options->>'realm' AS realm, c.enabled,
       (SELECT count(*) FROM accounts a WHERE a.channel_id=c.id AND a.enabled) AS enabled_accounts,
       cap.tasks, cap.daily, cap.checkin
FROM channels c
LEFT JOIN channel_caps cap ON cap.channel_id=c.id
WHERE c.id IN ('wb-cn', 'wb-intl')
ORDER BY c.id;

COMMIT;
