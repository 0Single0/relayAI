// 把参考项目（dynamic-island）的 SQLite 账本导入 relayAI 的 PostgreSQL。
//
// 用法：
//   node scripts/import-legacy-sqlite.mjs <legacy.db> [--dry-run] [--out dump.sql]
//
// 为什么用 Node 而不是 Go：读 SQLite 要么用 cgo（mattn/go-sqlite3，得装 MSVC），
// 要么加一个纯 Go 驱动（modernc.org/sqlite，几十 MB 依赖）。Node 22 自带
// `node:sqlite`，而 psql CLI 已经在这台机器上（导入目标就是它），
// 于是"读 SQLite → 生成 SQL → 交给 psql"这条链不需要新增任何依赖。
//
// 为什么不动参考项目的库：全程 `readOnly: true` 打开，SPEC 要求迁移源保持原样。
//
// ---------------------------------------------------------------------------
// 迁移范围与**明确不迁**的东西
// ---------------------------------------------------------------------------
//
// | 老表 | 迁徙情况 |
// | --- | --- |
// | `accounts` | 迁 uid / nickname / enabled / max_inflight / model_rename / source / added_at |
// | `accounts.credential` | **不迁**（见下） |
// | `usage` | 全量迁，`ts` 从 epoch 毫秒转 `timestamptz` |
// | `affinity` | **不迁**（纯运行态，TTL 30 分钟，迁过去全是过期行） |
//
// 凭据为什么不迁：老库里的 `credential` 是用参考项目**自己那把主密钥**
// （存在系统凭据库里的随机 32 字节）加密的 AES-GCM 密文。relayAI 的主密钥
// 来自 `RELAYAI_MASTER_KEY` 环境变量，两者无关，解不开。硬搬密文过去，
// 账号会显示"有凭据"但每次调用都解密失败 —— 比"显示没有凭据"更坏，
// 因为后者会直接提示用户重新登录。所以这里置 `credential_cipher = NULL`，
// 导入后需要重新登录一次。
//
// 渠道映射：只迁**能对上适配器**的渠道。对不上的（老库里的 `codearts`、
// 中文渠道名、拒绝行用的 `-`）分行处理，并且把处理结果打在报告里，
// 不静默丢数据 —— "导完发现少了 500 条"是最难查的一类问题。

import { writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";

const [, , legacyPath, ...rest] = process.argv;
if (!legacyPath) {
  console.error("用法: node scripts/import-legacy-sqlite.mjs <legacy.db> [--dry-run] [--out dump.sql]");
  process.exit(1);
}

const dryRun = rest.includes("--dry-run");
const outIndex = rest.indexOf("--out");
const outFile = outIndex >= 0 ? rest[outIndex + 1] : "legacy-import.sql";

/** 老库 `channel_id` → relayAI 渠道实例。 */
const CHANNEL_MAP = {
  "wb-cn": { id: "wb-cn", exists: true },
  "trae-cn": { id: "trae-cn", exists: true },
  // 参考项目里的 CodeArts 在 relayAI 里用 custom 适配器承接（静态 AK/SK 凭据）。
  codearts: { id: "codearts", exists: false, adapter: "custom", name: "CodeArts" },
};

/** 拒绝行的 `channel_id`：老库写 `-`，没有渠道归属。 */
const REJECTION_CHANNEL = "-";

const sqlString = (value) =>
  value === null || value === undefined ? "NULL" : `'${String(value).replaceAll("'", "''")}'`;

const sqlNumber = (value) =>
  value === null || value === undefined ? "NULL" : Number(value);

const sqlBool = (value) => (Number(value) ? "true" : "false");

/** epoch 毫秒 → timestamptz 表达式。 */
const sqlTs = (ms) =>
  ms === null || ms === undefined
    ? "NULL"
    : `to_timestamp(${Number(ms)} / 1000.0)`;

const db = new DatabaseSync(legacyPath, { readOnly: true });

const legacyAccounts = db
  .prepare(
    `SELECT channel_id, uid, nickname, enabled, max_inflight,
            model_rename, source, added_at
       FROM accounts`,
  )
  .all();

const legacyUsage = db
  .prepare(
    `SELECT rowid AS legacy_rowid,
            ts, model, channel_id, account_uid, status, stream, attempts,
            prompt_tokens, completion_tokens, reasoning_tokens, cached_tokens,
            total_tokens, credit, ttft_ms, total_ms, effort, estimated,
            upstream_model, error_detail
       FROM usage ORDER BY ts, rowid`,
  )
  .all();

db.close();

const skipped = [];
const channelsToCreate = new Map();
const lines = [];

lines.push("-- 由 scripts/import-legacy-sqlite.mjs 生成。");
lines.push("-- 幂等：账号按 (channel_id, uid) 去重，用量按 request_id = 'legacy-<rowid>' 去重。");
lines.push("BEGIN;");
lines.push("");

// ---- 渠道 ----
for (const account of legacyAccounts) {
  const mapped = CHANNEL_MAP[account.channel_id];
  if (!mapped) {
    skipped.push(`账号 ${account.channel_id}/${account.uid}：渠道没有对应适配器`);
    continue;
  }
  if (!mapped.exists) {
    channelsToCreate.set(mapped.id, mapped);
  }
}

for (const channel of channelsToCreate.values()) {
  lines.push(
    `INSERT INTO channels (id, name, adapter, enabled, priority, weight, options)` +
      ` VALUES (${sqlString(channel.id)}, ${sqlString(channel.name)},` +
      ` ${sqlString(channel.adapter)}, true, 100, 1, '{}'::jsonb)` +
      ` ON CONFLICT (id) DO NOTHING;`,
  );
}
lines.push("");

// ---- 账号 ----
let accountCount = 0;
for (const account of legacyAccounts) {
  const mapped = CHANNEL_MAP[account.channel_id];
  if (!mapped) {
    continue;
  }

  // model_rename 在老库里是非空 JSON 文本，迁过去要保持原样。
  const rename = account.model_rename === null ? "{}" : account.model_rename;

  lines.push(
    `INSERT INTO accounts` +
      ` (channel_id, uid, nickname, enabled, credential_cipher,` +
      `  max_inflight, model_rename, source, added_at)` +
      ` VALUES (${sqlString(mapped.id)}, ${sqlString(account.uid)},` +
      `  ${sqlString(account.nickname ?? "")}, ${sqlBool(account.enabled)},` +
      `  NULL, ${sqlNumber(account.max_inflight)},` +
      `  ${sqlString(rename)}::jsonb, ${sqlString(account.source ?? "oauth")},` +
      `  ${sqlTs(account.added_at)})` +
      ` ON CONFLICT (channel_id, uid) DO NOTHING;`,
  );
  accountCount += 1;
}
lines.push("");

// ---- 用量 ----
//
// `request_id` 老库里没有：用 `legacy-<rowid>` 造一个**稳定**的唯一键。
//
// 必须用 `rowid` 而不是循环序号：序号取决于查询结果的排列，一旦排序不稳定
// （同一毫秒有两条时 SQLite 的返回顺序不保证），同样的行会拿到不同的 id，
// 于是"再导一次"变成"再插一遍"——实测第一次 11877 行、第二次变成 11879 行。
// `rowid` 是老库的物理行号，同一行永远拿到同一个值。
//
// `is_rejection` 从 `status = 'rejected'` 派生 —— 老库正是靠 `channel_id = '-'`
// 隐式表达的，新库改成显式布尔列（SPEC 5.5）。
//
// `protocol` 老库没有记录这三协议维度（当年只有 Chat Completions），
// 全部落 `chat`。
let usageCount = 0;
for (const row of legacyUsage) {
  const mapped = CHANNEL_MAP[row.channel_id];
  const isRejection = row.status === "rejected";

  if (!mapped && !isRejection && row.channel_id !== REJECTION_CHANNEL) {
    skipped.push(`用量 ${row.channel_id} @${row.ts}：渠道没有对应适配器`);
    continue;
  }

  const channelId = mapped ? mapped.id : REJECTION_CHANNEL;
  const requestId = `legacy-${row.legacy_rowid}`;
  const credit = row.credit === null ? 0 : Number(row.credit);

  // 用 `WHERE NOT EXISTS` 而不是 `ON CONFLICT`：`usage_logs.request_id` 上没有
  // 唯一约束（拒绝行的 id 是 `rej-<毫秒>`，同一毫秒可能撞，不适合加唯一索引）。
  // 没有约束却写 `ON CONFLICT (request_id)` 会直接报错，整个导入回滚。
  lines.push(
    `INSERT INTO usage_logs` +
      ` (ts, request_id, key_id, channel_id, account_uid, protocol, model,` +
      `  upstream_model, stream, status, is_rejection, attempts,` +
      `  prompt_tokens, completion_tokens, reasoning_tokens, cached_tokens,` +
      `  total_tokens, usage_estimated, credit, upstream_cost, billed_cost,` +
      `  ttft_ms, total_ms, effort, error_detail)` +
      ` SELECT ${sqlTs(row.ts)}, ${sqlString(requestId)}, NULL,` +
      `  ${sqlString(channelId)}, ${sqlString(row.account_uid)}, 'chat',` +
      `  ${sqlString(row.model)}, ${sqlString(row.upstream_model)},` +
      `  ${sqlBool(row.stream)}, ${sqlString(row.status)}, ${isRejection ? "true" : "false"},` +
      `  ${sqlNumber(row.attempts) || 1},` +
      `  ${sqlNumber(row.prompt_tokens) || 0}, ${sqlNumber(row.completion_tokens) || 0},` +
      `  ${sqlNumber(row.reasoning_tokens) || 0}, ${sqlNumber(row.cached_tokens) || 0},` +
      `  ${sqlNumber(row.total_tokens) || 0}, ${sqlBool(row.estimated)},` +
      `  ${credit}, ${credit}, ${credit},` +
      `  ${sqlNumber(row.ttft_ms)}, ${sqlNumber(row.total_ms) || 0},` +
      `  ${sqlString(row.effort)}, ${sqlString(row.error_detail)}` +
      ` WHERE NOT EXISTS (SELECT 1 FROM usage_logs WHERE request_id = ${sqlString(requestId)});`,
  );
  usageCount += 1;
}

lines.push("");
lines.push("COMMIT;");

writeFileSync(outFile, lines.join("\n"), "utf8");

const report = {
  老库账号: legacyAccounts.length,
  将要导入账号: accountCount,
  老库用量: legacyUsage.length,
  将要导入用量: usageCount,
  新建渠道: [...channelsToCreate.keys()],
  跳过: skipped.length,
  跳过明细: skipped.slice(0, 20),
  输出文件: outFile,
};

console.log(JSON.stringify(report, null, 2));

if (dryRun) {
  console.log("\n--dry-run：只生成了 SQL，未写入数据库。");
  process.exit(0);
}

const psql = process.env.PSQL_BIN ?? "psql";
const result = spawnSync(
  psql,
  [
    process.env.PGHOST ? "-h" : "-h",
    process.env.PGHOST ?? "127.0.0.1",
    "-U",
    process.env.PGUSER ?? "postgres",
    "-d",
    process.env.PGDATABASE ?? "relayai",
    "-v",
    "ON_ERROR_STOP=1",
    "-f",
    outFile,
  ],
  { stdio: "inherit", env: { ...process.env, PGCLIENTENCODING: "UTF8" } },
);

if (result.error) {
  console.error(`\n调用 psql 失败：${result.error.message}`);
  console.error(`可以手工执行：${psql} -f ${outFile}`);
  process.exit(1);
}

process.exit(result.status ?? 0);
