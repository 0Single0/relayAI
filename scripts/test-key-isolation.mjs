// Key 渠道隔离测试（SPEC 12 / M7 验收）
//
// 验证：
//   1. 白名单生效：Key A 只能用 wb-cn，请求 trae 独占模型 → 403
//   2. 模型清单按 Key 过滤：不该出现该 Key 用不了的模型
//   3. 渠道是 1 对 1 必填项，未绑渠道的 Key 会被拒绝
//   4. 403 不是 503（权限问题不能伪装成容量问题）
//
// 用法：node scripts/test-key-isolation.mjs <gatewayBase> <adminBase>

const gateway = (process.argv[2] ?? "http://127.0.0.1:18788").replace(/\/$/, "");
const admin = (process.argv[3] ?? "http://127.0.0.1:8888/api/v1").replace(/\/$/, "");

let pass = 0;
let fail = 0;
const failures = [];

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

/*
  登录拿 token：密钥接口要求管理员身份（它能创建"能花钱"的凭据）。
*/
let adminToken = "";
async function loginAdmin() {
  const response = await fetch(`${admin}/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      email: process.env.RELAYAI_ADMIN_EMAIL ?? "admin@relayai.local",
      password: process.env.RELAYAI_ADMIN_PASSWORD ?? "relayai2026",
    }),
  });
  const payload = await response.json();
  adminToken = payload?.data?.token ?? "";
  if (!adminToken) {
    throw new Error(`登录失败：${JSON.stringify(payload).slice(0, 200)}`);
  }
}

async function adminApi(method, path, body) {
  const options = { method, headers: { "Content-Type": "application/json" } };
  if (adminToken) {
    options.headers.Authorization = `Bearer ${adminToken}`;
  }
  if (body !== undefined) {
    options.body = JSON.stringify(body);
  }
  const response = await fetch(`${admin}${path}`, options);
  const payload = await response.json();
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}: ${payload?.msg ?? ""}`);
  }
  return payload;
}

async function chat(key, model) {
  const response = await fetch(`${gateway}/v1/chat/completions`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      ...(key ? { Authorization: `Bearer ${key}` } : {}),
    },
    body: JSON.stringify({ model, messages: [{ role: "user", content: "hi" }] }),
  });
  const text = await response.text();
  return { status: response.status, body: text };
}

async function models(key) {
  const response = await fetch(`${gateway}/v1/models`, {
    headers: key ? { Authorization: `Bearer ${key}` } : {},
  });
  const payload = await response.json();
  return { status: response.status, ids: (payload.data ?? []).map((m) => m.id) };
}

// 准备：两个 Key，各自绑一个渠道（fake 与 codearts），再加一条负向断言。
const stamp = Date.now();
const restrictedKey = { name: `隔离测试-限制-${stamp}`, id: null, plaintext: null };
const openKey = { name: `隔离测试-不限-${stamp}`, id: null, plaintext: null };

await loginAdmin();

console.log("=== 准备测试 Key ===");

await step("创建受限 Key（channelAllow=[fake]）", async () => {
  const r = await adminApi("POST", "/keys", {
    name: restrictedKey.name,
    channelAllow: ["fake"],
    modelAllow: [],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
    expiresAt: null,
  });
  restrictedKey.id = r.data.key.id;
  restrictedKey.plaintext = r.data.plaintext;
  assert(restrictedKey.plaintext.startsWith("sk-"), "明文格式不对");
});

await step("创建第二个 Key（绑另一个渠道，模型不限）", async () => {
  const r = await adminApi("POST", "/keys", {
    name: openKey.name,
    // 渠道**必须**选一个；这里用 codearts 验证"换渠道"确实生效。
    channelAllow: ["codearts"],
    modelAllow: [],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
    expiresAt: null,
  });
  openKey.id = r.data.key.id;
  openKey.plaintext = r.data.plaintext;
});

// 负向断言：渠道是 1 对 1 的必填项，留空必须被拒绝。
// 这条守住"没有全部渠道这个概念"—— 否则前端限制被绕过就又回到"任意通道"。
await step("拒绝创建未绑渠道的 Key", async () => {
  const response = await fetch(`${admin}/keys`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${adminToken}` },
    body: JSON.stringify({
      name: `隔离测试-无渠道-${stamp}`,
      channelAllow: [],
      modelAllow: [],
      rpmLimit: null,
      dailyTokenLimit: null,
      maxConcurrency: null,
      expiresAt: null,
    }),
  });
  assert(response.status === 400, `应当返回 400，实际 ${response.status}`);
});

console.log("\n=== 鉴权 ===");

await step("无 Key 请求被拒（库里已有 Key）", async () => {
  const r = await chat(null, "fake-model");
  assert(r.status === 401, `实际 ${r.status}`);
});

await step("错误 Key 被拒", async () => {
  const r = await chat("sk-deadbeef", "fake-model");
  assert(r.status === 401, `实际 ${r.status}`);
});

await step("正确 Key 可以请求", async () => {
  const r = await chat(restrictedKey.plaintext, "fake-model");
  assert(r.status === 200, `实际 ${r.status}: ${r.body.slice(0, 150)}`);
});

console.log("\n=== 渠道白名单 ===");

await step("受限 Key 请求不在白名单的渠道 → 403", async () => {
  // 先建一个 trae 渠道 + 账号，并让它提供 fake-model 之外的模型。
  // 简化做法：直接把受限 Key 的 channelAllow 改成不存在的渠道，再请求。
  await adminApi("PATCH", `/keys/${restrictedKey.id}`, {
    name: restrictedKey.name,
    channelAllow: ["wb-cn"], // fake 渠道不在其中
    modelAllow: [],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
  });

  const r = await chat(restrictedKey.plaintext, "fake-model");
  assert(r.status === 403, `应 403（权限），实际 ${r.status}: ${r.body.slice(0, 200)}`);
  assert(
    !/503/.test(String(r.status)),
    "403 不能伪装成 503 —— 那会让用户去查账号冷却",
  );
});

await step("403 的错误消息说明了允许的渠道", async () => {
  const r = await chat(restrictedKey.plaintext, "fake-model");
  const payload = JSON.parse(r.body);
  const message = payload?.error?.message ?? "";
  assert(message.includes("wb-cn"), `消息里应提到允许的渠道，实际：${message}`);
});

await step("恢复白名单后可正常请求", async () => {
  await adminApi("PATCH", `/keys/${restrictedKey.id}`, {
    name: restrictedKey.name,
    channelAllow: ["fake"],
    modelAllow: [],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
  });

  const r = await chat(restrictedKey.plaintext, "fake-model");
  assert(r.status === 200, `实际 ${r.status}`);
});

console.log("\n=== 模型白名单 ===");

await step("模型不在白名单 → 403", async () => {
  await adminApi("PATCH", `/keys/${restrictedKey.id}`, {
    name: restrictedKey.name,
    channelAllow: ["fake"],
    modelAllow: ["some-other-model"],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
  });

  const r = await chat(restrictedKey.plaintext, "fake-model");
  assert(r.status === 403, `应 403，实际 ${r.status}: ${r.body.slice(0, 200)}`);
});

await step("模型白名单命中时可请求", async () => {
  await adminApi("PATCH", `/keys/${restrictedKey.id}`, {
    name: restrictedKey.name,
    channelAllow: ["fake"],
    modelAllow: ["fake-model"],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
  });

  const r = await chat(restrictedKey.plaintext, "fake-model");
  assert(r.status === 200, `实际 ${r.status}`);
});

console.log("\n=== /v1/models 按 Key 过滤 ===");

await step("受限 Key 的模型清单含 fake-model", async () => {
  const r = await models(restrictedKey.plaintext);
  assert(r.status === 200, `实际 ${r.status}`);
  assert(r.ids.includes("fake-model"), `清单：${r.ids.join(",")}`);
});

await step("模型白名单不含 fake-model 时，清单里也不出现", async () => {
  await adminApi("PATCH", `/keys/${restrictedKey.id}`, {
    name: restrictedKey.name,
    channelAllow: ["fake"],
    modelAllow: ["some-other-model"],
    rpmLimit: null,
    dailyTokenLimit: null,
    maxConcurrency: null,
  });

  const r = await models(restrictedKey.plaintext);
  assert(!r.ids.includes("fake-model"), `不该出现 fake-model，实际：${r.ids.join(",")}`);
});

console.log("\n=== 绑不同渠道的 Key ===");

// 这把 Key 绑的是 codearts（没有 fake-model），所以打 fake-model 应当被拒。
// 这正好证明"渠道 1 对 1"是真的生效，而不是只写进库里没人看。
await step("绑 codearts 的 Key 打 fake-model 被拒", async () => {
  const r = await chat(openKey.plaintext, "fake-model");
  assert(r.status !== 200, `不应放行，实际 ${r.status}`);
});

await step("绑 codearts 的 Key 能拿到模型清单", async () => {
  const r = await models(openKey.plaintext);
  assert(r.status === 200, `实际 ${r.status}`);
});

console.log("\n=== 清理 ===");
await step("删除测试 Key", async () => {
  await adminApi("DELETE", `/keys/${restrictedKey.id}`);
  await adminApi("DELETE", `/keys/${openKey.id}`);
});

console.log(`\n合计：通过 ${pass} 项，失败 ${fail} 项`);
if (fail > 0) {
  console.log("\n失败项：");
  for (const item of failures) {
    console.log(`  - ${item.name}: ${item.message}`);
  }
  process.exit(1);
}
