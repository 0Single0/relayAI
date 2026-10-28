// API 冒烟测试
//
// 覆盖 SPEC 7.2 的主要端点。用 Node 而不是 PowerShell：
// Windows PowerShell 5 读无 BOM 的 UTF-8 脚本会按 ANSI 解码，中文全变乱码。
//
// 用法：node scripts/smoke-api.mjs [baseUrl]
// 前置：Go 后端在 8888，Rust 网关在 18788，PG 已迁移。

const base = (process.argv[2] ?? "http://127.0.0.1:8888/api/v1").replace(/\/$/, "");

let pass = 0;
let fail = 0;
const failures = [];

/*
  登录拿 token。

  为什么需要：密钥、结算、模型价格这些接口现在要求管理员身份 ——
  它们是"能花钱 / 能改价"的操作，不该匿名可调。
  测试必须带真实凭据，否则验的是"未登录也能调"，那本身就是错的。
*/
let accessToken = "";
async function login() {
  const response = await fetch(`${base}/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      email: process.env.RELAYAI_ADMIN_EMAIL ?? "admin@relayai.local",
      password: process.env.RELAYAI_ADMIN_PASSWORD ?? "relayai2026",
    }),
  });
  const payload = await response.json();
  accessToken = payload?.data?.token ?? "";
  if (!accessToken) {
    throw new Error(`登录失败：${JSON.stringify(payload).slice(0, 200)}`);
  }
}

async function api(method, path, body) {
  const options = { method, headers: { "Content-Type": "application/json" } };
  if (accessToken) {
    options.headers.Authorization = `Bearer ${accessToken}`;
  }
  if (body !== undefined) {
    options.body = JSON.stringify(body);
  }

  const response = await fetch(`${base}${path}`, options);
  const text = await response.text();

  let payload = null;
  try {
    payload = text ? JSON.parse(text) : null;
  } catch {
    payload = { raw: text };
  }

  if (!response.ok) {
    const error = new Error(`HTTP ${response.status}: ${payload?.msg ?? text.slice(0, 120)}`);
    error.status = response.status;
    error.payload = payload;
    throw error;
  }

  return payload;
}

async function step(name, run) {
  try {
    await run();
    console.log(`  ok   ${name}`);
    pass += 1;
  } catch (error) {
    console.log(`  FAIL ${name} —— ${error.message}`);
    failures.push({ name, message: error.message });
    fail += 1;
  }
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

await login();

console.log("=== 健康检查 ===");
await step("GET /health 返回 database=up", async () => {
  const r = await api("GET", "/health");
  assert(r.data.database === "up", `database=${r.data.database}`);
});

console.log("\n=== 渠道 CRUD ===");
await step("GET /channels 有 fake 渠道", async () => {
  const r = await api("GET", "/channels");
  assert(
    r.data.some((c) => c.id === "fake"),
    "缺少 fake 渠道",
  );
});

await step("POST /channels 新建 custom 渠道", async () => {
  const r = await api("POST", "/channels", {
    id: "smoke-custom",
    name: "冒烟测试供应商",
    adapter: "custom",
    options: { base_url: "https://example.com/v1", protocol: "openai-chat" },
    channelRatio: 1.5,
  });
  assert(r.data.id === "smoke-custom", `返回 id=${r.data.id}`);
});

await step("PATCH /channels 改名", async () => {
  const r = await api("PATCH", "/channels/smoke-custom", { name: "改名后" });
  assert(r.data.name === "改名后", `name=${r.data.name}`);
});

await step("DELETE /channels 删除", async () => {
  await api("DELETE", "/channels/smoke-custom");
  const r = await api("GET", "/channels");
  assert(!r.data.some((c) => c.id === "smoke-custom"), "未删除");
});

console.log("\n=== 密钥 ===");
let newKeyId = null;

await step("POST /keys 返回一次性明文", async () => {
  const r = await api("POST", "/keys", {
    name: "冒烟测试",
    channelAllow: ["fake"],
    modelAllow: [],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
    expiresAt: null,
  });
  assert(r.data.plaintext.startsWith("sk-"), "明文格式不对");
  assert(r.data.key.keyPrefix.endsWith("****"), "前缀格式不对");
  newKeyId = r.data.key.id;
});

// 密钥明文**按设计可反复读取**（对齐 sub2api）：库里存一份明文，
// 列表接口返回它，界面随时能复制。所以这里断言的是"能读到"，
// 而不是旧设计的"不含明文"。
//
// 安全性说明：这确实降低了"库被读走"的门槛，但不影响**上游凭据**——
// 账号的 access_token / API Key 仍由主密钥加密，主密钥只在网关进程里。
// 下游 Key 泄露的处置是"在界面上轮换"，不需要碰上游账号。
await step("GET /keys 返回明文供复制", async () => {
  const r = await api("GET", "/keys");
  const target = (r.data ?? []).find((item) => item.id === newKeyId);
  assert(target, "列表里找不到刚建的密钥");
  assert(
    typeof target.plaintext === "string" && target.plaintext.startsWith("sk-"),
    "新建的密钥应当能读到明文",
  );
});

await step("POST /keys/{id}/rotate 返回新明文", async () => {
  const r = await api("POST", `/keys/${newKeyId}/rotate`);
  assert(r.data.plaintext.startsWith("sk-"), "明文格式不对");
});

await step("DELETE /keys/{id}", async () => {
  await api("DELETE", `/keys/${newKeyId}`);
});

console.log("\n=== 账号 ===");
await step("GET /accounts 分页结构", async () => {
  const r = await api("GET", "/accounts?page=1&pageSize=10");
  assert(typeof r.data.total === "number", "缺少 total");
  assert(Array.isArray(r.data.list), "缺少 list");
});

await step("PATCH /accounts/{id} 改昵称", async () => {
  const list = (await api("GET", "/accounts")).data.list;
  const target = list.find((a) => a.channelId === "fake");
  assert(target, "找不到 fake 账号");
  const r = await api("PATCH", `/accounts/${target.id}`, { nickname: "改名账号" });
  assert(r.data.nickname === "改名账号", `nickname=${r.data.nickname}`);
});

console.log("\n=== 用量 ===");
await step("GET /usage 分页", async () => {
  const r = await api("GET", "/usage?page=1&pageSize=10");
  assert(typeof r.data.total === "number", "缺少 total");
});

await step("GET /usage/summary 排除拒绝行", async () => {
  const r = await api("GET", "/usage/summary?groupBy=channel");
  assert(r.data.totals, "缺少 totals");
  const bad = r.data.buckets.find((b) => b.key === "-");
  assert(!bad, "统计里出现了拒绝行分组");
});

await step("GET /usage/series", async () => {
  const r = await api("GET", "/usage/series?bucket=hour");
  assert(Array.isArray(r.data), "缺少 data");
});

console.log("\n=== 任务 ===");
await step("GET /tasks/channels 只返回有能力的渠道", async () => {
  const r = await api("GET", "/tasks/channels");
  assert(Array.isArray(r.data), "缺少 data");

  // fake 没有运营能力，不该出现；workbuddy/trae 有，必须出现。
  assert(
    !r.data.some((c) => c.id === "fake"),
    "没有能力的渠道不该出现在任务菜单里",
  );

  for (const channel of r.data) {
    const caps = channel.caps ?? {};
    assert(
      caps.tasks || caps.daily || caps.checkin,
      `渠道 ${channel.id} 三项能力全 false，不该出现`,
    );
  }
});

await step("GET /tasks/runs 分页", async () => {
  const r = await api("GET", "/tasks/runs?page=1&pageSize=10");
  assert(typeof r.data.total === "number", "缺少 total");
});

console.log("\n=== 系统 ===");
await step("GET /status 代理网关", async () => {
  const r = await api("GET", "/status");
  assert(typeof r.data.running === "boolean", "缺少 running");
});

await step("GET /settings 字段齐全且合法", async () => {
  const r = await api("GET", "/settings");
  // 不断言具体默认值：上一轮 PATCH 会改掉它（测试之间是有状态的）。
  // 断言"字段存在且落在合法范围"才是这个接口的契约。
  assert(
    typeof r.data.taskRetentionDays === "number" &&
      r.data.taskRetentionDays >= 7 &&
      r.data.taskRetentionDays <= 365,
    `retention=${r.data.taskRetentionDays} 超出 7–365`,
  );
  assert(typeof r.data.taskScheduleEnabled === "boolean", "缺少 taskScheduleEnabled");
  assert(Array.isArray(r.data.taskScheduleHours), "缺少 taskScheduleHours");
  assert(typeof r.data.gatewayPort === "number", "缺少 gatewayPort");
});

await step("PATCH /settings 保留天数越界被拒", async () => {
  try {
    await api("PATCH", "/settings", { taskRetentionDays: 999 });
    throw new Error("应当返回 400");
  } catch (error) {
    assert(error.status === 400, `应 400，实际 ${error.status}`);
  }
});

await step("PATCH /settings 合法值保存", async () => {
  const r = await api("PATCH", "/settings", {
    taskRetentionDays: 120,
    taskScheduleEnabled: true,
    taskScheduleHours: [9, 21],
    taskPerAccountGapMs: 800,
    taskDailyGapMs: 1200,
    taskReportGapMs: 1100,
    gatewayPort: 18788,
    gatewayBind: "127.0.0.1",
    logRetentionDays: 30,
  });
  assert(r.data.taskRetentionDays === 120, `retention=${r.data.taskRetentionDays}`);
});

console.log("\n=== 错误处理 ===");
await step("GET /channels/{不存在} 返回 404", async () => {
  try {
    await api("GET", "/channels/nope");
    throw new Error("应当 404");
  } catch (error) {
    assert(error.status === 404, `应 404，实际 ${error.status}`);
  }
});

await step("GET /accounts/{非法id} 返回 400", async () => {
  try {
    await api("GET", "/accounts/abc");
    throw new Error("应当 400");
  } catch (error) {
    assert(error.status === 400, `应 400，实际 ${error.status}`);
  }
});

console.log(`\n合计：通过 ${pass} 项，失败 ${fail} 项`);
if (fail > 0) {
  console.log("\n失败项：");
  for (const item of failures) {
    console.log(`  - ${item.name}: ${item.message}`);
  }
  process.exit(1);
}
