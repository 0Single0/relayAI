// 三协议联调测试（SPEC M5 验收）
//
// 验证网关的三条入口都能用：
//   1. POST /v1/chat/completions    OpenAI Chat Completions
//   2. POST /v1/messages            Anthropic Messages
//   3. POST /v1/responses           OpenAI Responses（无状态）
//
// 每个协议都测流式与非流式，并检查协议特有的字段与事件序列。
// 用 fake 渠道，不碰任何真实上游。
//
// **关于鉴权**：网关有一条"库里没有任何 Key 就免鉴权"的本地便利规则。
// 早期这脚本靠它跑，但一旦库里有 Key（比如刚建过一把），
// 所有请求就会 401 —— 测试结果取决于环境，而不是代码。
// 所以这里**自己建一把临时 Key**、跑完删掉，不依赖那个模式。
//
// 用法：node scripts/test-protocols.mjs [gatewayBaseUrl] [adminBaseUrl]

const base = (process.argv[2] ?? "http://127.0.0.1:18788").replace(/\/$/, "");
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
  登录拿 token。

  密钥接口现在要求管理员身份（它能创建"能花钱"的凭据，不该匿名可调）。
  所以这里先登录，把 token 带上。
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

/** 建一把临时 Key（管理面），返回明文与 id。 */
async function createTempKey() {
  const response = await fetch(`${admin}/keys`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${adminToken}`,
    },
    // 渠道是 1 对 1 必填项：这些协议用例全部打 fake 渠道，所以绑 fake。
    body: JSON.stringify({
      name: "protocol-test-temp",
      channelAllow: ["fake"],
      modelAllow: [],
    }),
  });
  const payload = await response.json();
  // 响应结构是 `{ data: { key: {...}, plaintext: "sk-..." } }`：
  // 明文只在这一次返回，务必从 `data.plaintext` 取，不要翻 `data.key`。
  const plaintext = payload?.data?.plaintext;
  const id = payload?.data?.key?.id;
  if (!plaintext || !id) {
    throw new Error(`建临时 Key 失败：${JSON.stringify(payload).slice(0, 200)}`);
  }
  return { plaintext, id };
}

async function deleteTempKey(id) {
  await fetch(`${admin}/keys/${id}`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${adminToken}` },
  }).catch(() => {});
}

await loginAdmin();
const tempKey = await createTempKey();
const authHeaders = {
  "Content-Type": "application/json",
  Authorization: `Bearer ${tempKey.plaintext}`,
};

async function postJson(path, body) {
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers: authHeaders,
    body: JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}: ${text.slice(0, 200)}`);
  }
  return JSON.parse(text);
}

async function postStream(path, body) {
  const response = await fetch(`${base}${path}`, {
    method: "POST",
    headers: authHeaders,
    body: JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}: ${text.slice(0, 200)}`);
  }
  return text;
}

const messages = [{ role: "user", content: "你好" }];

console.log("=== Chat Completions（OpenAI）===");

await step("非流式返回 chat.completion", async () => {
  const r = await postJson("/v1/chat/completions", { model: "fake-model", messages });
  assert(r.object === "chat.completion", `object=${r.object}`);
  assert(r.choices?.[0]?.message?.role === "assistant", "缺少 assistant 消息");
  assert(typeof r.usage?.total_tokens === "number", "缺少 usage");
});

await step("流式以 data: [DONE] 结尾", async () => {
  const text = await postStream("/v1/chat/completions", {
    model: "fake-model",
    messages,
    stream: true,
  });
  assert(text.includes("chat.completion.chunk"), "缺少 chunk 对象");
  assert(text.trimEnd().endsWith("data: [DONE]"), "缺少 [DONE] 结束标记");
});

await step("模型不存在返回 400", async () => {
  try {
    await postJson("/v1/chat/completions", { model: "no-such-model", messages });
    throw new Error("应当返回 400");
  } catch (error) {
    assert(/HTTP 400/.test(error.message), `实际：${error.message}`);
  }
});

console.log("\n=== Anthropic Messages ===");

await step("非流式返回 message 对象", async () => {
  const r = await postJson("/v1/messages", {
    model: "fake-model",
    max_tokens: 64,
    messages,
  });
  assert(r.type === "message", `type=${r.type}`);
  assert(r.role === "assistant", `role=${r.role}`);
  assert(Array.isArray(r.content), "content 应为数组");
  assert(typeof r.usage?.input_tokens === "number", "缺少 input_tokens");
  assert(typeof r.usage?.output_tokens === "number", "缺少 output_tokens");
});

await step("流式事件序列完整", async () => {
  const text = await postStream("/v1/messages", {
    model: "fake-model",
    max_tokens: 64,
    messages,
    stream: true,
  });

  // Anthropic 的 SSE 用 event: 行区分事件类型（不是 data 里的 type 字段）。
  assert(text.includes("event: message_start"), "缺少 message_start");
  assert(text.includes("event: content_block_start"), "缺少 content_block_start");
  assert(text.includes("event: content_block_delta"), "缺少 content_block_delta");
  assert(text.includes("event: content_block_stop"), "缺少 content_block_stop");
  assert(text.includes("event: message_delta"), "缺少 message_delta");
  assert(text.includes("event: message_stop"), "缺少 message_stop");
});

await step("缺少 max_tokens 时仍能处理（宽容接受）", async () => {
  // Anthropic 规范要求 max_tokens 必填，但网关刻意宽容：缺省时交给上游判断，
  // 而不是在本地 400。理由是多一层校验就多一处"客户端能用、网关拒了"的分歧。
  // 这里断言的是"不因为缺字段而崩"，不是"必须 400"。
  const r = await postJson("/v1/messages", { model: "fake-model", messages });
  assert(r.type === "message", `type=${r.type}`);
});

console.log("\n=== Responses（OpenAI 无状态）===");

await step("非流式返回 response 对象", async () => {
  const r = await postJson("/v1/responses", {
    model: "fake-model",
    input: "你好",
    max_output_tokens: 64,
  });
  assert(r.object === "response", `object=${r.object}`);
  assert(r.status === "completed", `status=${r.status}`);
  assert(Array.isArray(r.output), "output 应为数组");
});

await step("流式包含 response.completed", async () => {
  const text = await postStream("/v1/responses", {
    model: "fake-model",
    input: "你好",
    max_output_tokens: 64,
    stream: true,
  });
  assert(text.includes("response.created"), "缺少 response.created");
  assert(text.includes("response.completed"), "缺少 response.completed");
});

await step("previous_response_id 被拒绝（无状态网关）", async () => {
  try {
    await postJson("/v1/responses", {
      model: "fake-model",
      input: "继续",
      previous_response_id: "resp_123",
    });
    throw new Error("应当返回 400");
  } catch (error) {
    assert(/HTTP 400/.test(error.message), `实际：${error.message}`);
  }
});

console.log("\n=== 工具调用（跨协议）===");

const tools = [
  {
    type: "function",
    function: {
      name: "get_weather",
      description: "查询天气",
      parameters: { type: "object", properties: { city: { type: "string" } } },
    },
  },
];

await step("Chat Completions 返回 tool_calls（含拼好的 arguments）", async () => {
  const r = await postJson("/v1/chat/completions", {
    model: "fake-model",
    messages,
    tools,
  });
  const message = r.choices?.[0]?.message;
  assert(message, "缺少 message");
  const calls = message.tool_calls ?? [];
  assert(calls.length > 0, `没有 tool_calls：${JSON.stringify(message)}`);
  assert(calls[0].type === "function", `type=${calls[0].type}`);
  assert(calls[0].function?.name === "get_weather", `name=${calls[0].function?.name}`);
  // 分片传的 arguments 必须被拼成一个**合法 JSON 字符串**，不是两段碎片。
  const args = calls[0].function?.arguments ?? "";
  assert(args === '{"city":"北京"}', `arguments 拼接错了：${args}`);
  assert(r.choices[0].finish_reason === "tool_calls", `finish=${r.choices[0].finish_reason}`);
});

await step("Chat Completions 多轮：回传工具结果后继续", async () => {
  const r = await postJson("/v1/chat/completions", {
    model: "fake-model",
    messages: [
      ...messages,
      {
        role: "assistant",
        content: null,
        tool_calls: [
          {
            id: "call_fake_1",
            type: "function",
            function: { name: "get_weather", arguments: '{"city":"北京"}' },
          },
        ],
      },
      { role: "tool", tool_call_id: "call_fake_1", content: "晴，26℃" },
    ],
    tools,
  });
  assert(r.choices?.[0]?.message, "多轮回传后没有 message");
});

await step("Anthropic 返回 tool_use 块", async () => {
  const r = await postJson("/v1/messages", {
    model: "fake-model",
    max_tokens: 64,
    messages,
    tools: [
      {
        name: "get_weather",
        description: "查询天气",
        input_schema: { type: "object", properties: { city: { type: "string" } } },
      },
    ],
  });
  assert(r.type === "message", `type=${r.type}`);
  const block = (r.content ?? []).find((item) => item.type === "tool_use");
  assert(block, `没有 tool_use 块：${JSON.stringify(r.content)}`);
  assert(block.name === "get_weather", `name=${block.name}`);
  // Anthropic 的 input 是**对象**（不是 JSON 字符串），必须真的解析过。
  assert(
    block.input && block.input.city === "北京",
    `input 未解析成对象：${JSON.stringify(block.input)}`,
  );
  assert(r.stop_reason === "tool_use", `stop_reason=${r.stop_reason}`);
});

await step("Responses 返回 function_call 项", async () => {
  const r = await postJson("/v1/responses", {
    model: "fake-model",
    input: "今天北京天气",
    tools: [
      {
        type: "function",
        name: "get_weather",
        description: "查询天气",
        parameters: { type: "object", properties: { city: { type: "string" } } },
      },
    ],
  });
  const call = (r.output ?? []).find((item) => item.type === "function_call");
  assert(call, `没有 function_call：${JSON.stringify(r.output)}`);
  assert(call.name === "get_weather", `name=${call.name}`);
  assert(call.arguments === '{"city":"北京"}', `arguments=${call.arguments}`);
});

console.log("\n=== /v1/models ===");

await step("返回 fake-model", async () => {
  const response = await fetch(`${base}/v1/models`, {
    headers: { Authorization: `Bearer ${tempKey.plaintext}` },
  });
  const r = await response.json();
  assert(r.object === "list", `object=${r.object}`);
  assert(
    r.data.some((m) => m.id === "fake-model"),
    `列表里没有 fake-model：${JSON.stringify(r.data.map((m) => m.id))}`,
  );
});

await deleteTempKey(tempKey.id);

console.log(`\n合计：通过 ${pass} 项，失败 ${fail} 项`);
if (fail > 0) {
  console.log("\n失败项：");
  for (const item of failures) {
    console.log(`  - ${item.name}: ${item.message}`);
  }
  process.exit(1);
}
