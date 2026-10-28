// 逐个访问路由，检查是否有 console 错误 / 未捕获异常 / 横向溢出。
//   node scripts/audit-routes.mjs <baseUrl>
//
// 加了登录守卫之后，未登录访问任何路由都会被重定向到 /login，
// 于是 `h1` 永远是"登录"、`#main-content` 永远不存在 —— 整轮审计会
// 因为"没登录"而假装通过（原来那次输出里 h1 是"无法访问此网站"，
// 就是因为本地根本没起服务，却没被判成失败）。
// 所以这里先拿一个真令牌塞进 localStorage，再开始逐页访问。
const [, , base = "http://localhost:4173", apiBase = "http://127.0.0.1:8888/api/v1"] =
  process.argv;

const ADMIN_EMAIL = "admin@relayai.local";
const ADMIN_PASSWORD = "relayai2026";

const routes = [
  "/dashboard",
  "/channels",
  "/channels/wb-cn",
  "/accounts",
  "/accounts/1",
  "/status",
  "/tasks/wb-cn",
  "/usage",
  "/proxies",
  "/ops",
  "/models",
  "/plaza",
  "/keys",
  "/users",
  "/redeem-codes",
  "/account",
  "/settings",
  "/does-not-exist",
];

/** 先登录拿令牌；失败就直接退出，不要静默跑一轮假审计。 */
const loginResponse = await fetch(`${apiBase}/auth/login`, {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ email: ADMIN_EMAIL, password: ADMIN_PASSWORD }),
});
const login = await loginResponse.json();
const token = login?.data?.token;
if (!token) {
  console.error("登录失败，无法审计登录后的页面：", JSON.stringify(login).slice(0, 300));
  process.exit(1);
}

const targets = await (await fetch("http://127.0.0.1:9333/json/list")).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("没有可用的 page target");

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
let consoleErrors = [];
let exceptions = [];

ws.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(JSON.stringify(msg.error))) : resolve(msg.result);
    return;
  }
  if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
    consoleErrors.push(msg.params.args.map((a) => a.value ?? a.description).join(" "));
  }
  if (msg.method === "Runtime.exceptionThrown") {
    exceptions.push(msg.params.exceptionDetails.text ?? "unknown exception");
  }
});

const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const messageId = ++id;
    pending.set(messageId, { resolve, reject });
    ws.send(JSON.stringify({ id: messageId, method, params }));
  });

await new Promise((resolve) => ws.addEventListener("open", resolve, { once: true }));
await send("Page.enable");
await send("Runtime.enable");

// 先把令牌写进 localStorage：同源策略要求必须先有一个该源的页面。
await send("Page.navigate", { url: `${base}/login` });
await new Promise((r) => setTimeout(r, 2500));
await send("Runtime.evaluate", {
  expression: `localStorage.setItem("relayai.auth.token", ${JSON.stringify(token)})`,
  returnByValue: true,
});

/*
  写完立刻验证：页面若还在加载，稍后的 bootstrap 会拿**空 token** 去问 /auth/me，
  失败后 clearAuthSession() 把刚写的令牌清掉 —— 整轮审计于是全被判成"在登录页"。
  这是脚本的时序问题，不是产品 bug；所以这里显式等到页面稳定再确认一次。
*/
for (let attempt = 0; attempt < 6; attempt += 1) {
  await new Promise((r) => setTimeout(r, 800));
  const check = await send("Runtime.evaluate", {
    expression: `localStorage.getItem("relayai.auth.token") ?? ""`,
    returnByValue: true,
  });
  if (check.result.value === token) {
    // 再确认一次（隔 800ms 仍在），排除"刚写完还没被清"的窗口。
    await new Promise((r) => setTimeout(r, 800));
    const again = await send("Runtime.evaluate", {
      expression: `localStorage.getItem("relayai.auth.token") ?? ""`,
      returnByValue: true,
    });
    if (again.result.value === token) break;
  }
  await send("Runtime.evaluate", {
    expression: `localStorage.setItem("relayai.auth.token", ${JSON.stringify(token)})`,
    returnByValue: true,
  });
}

const results = [];

for (const width of [1440, 390]) {
  await send("Emulation.setDeviceMetricsOverride", {
    width,
    height: width === 390 ? 844 : 900,
    deviceScaleFactor: 1,
    mobile: width === 390,
  });

  for (const route of routes) {
    consoleErrors = [];
    exceptions = [];
    await send("Page.navigate", { url: `${base}${route}` });
    // 等够：部分页面要先拉接口才渲染标题（模型广场在窄屏下更慢）。
    // 等够：bootstrap 要先问 /auth/me 才决定放行还是跳登录页。
    // 2600ms 在冷启动时不够（实测整轮被判成"跳到了登录页"）。
    await new Promise((r) => setTimeout(r, 4200));

    const measured = await send("Runtime.evaluate", {
      expression: `(() => ({
        scrollWidth: document.documentElement.scrollWidth,
        clientWidth: document.documentElement.clientWidth,
        title: document.title,
        h1: document.querySelector('h1')?.textContent ?? '',
        hasMain: !!document.querySelector('#main-content'),
      }))()`,
      returnByValue: true,
    });

    const value = measured.result.value;
    results.push({
      width,
      route,
      overflow: value.scrollWidth - value.clientWidth,
      title: value.title,
      h1: value.h1,
      hasMain: value.hasMain,
      consoleErrors: [...consoleErrors],
      exceptions: [...exceptions],
    });
  }
}

const PUBLIC_ROUTES = new Set(["/plaza"]);

const problems = results.filter(
  (r) =>
    r.overflow > 0 ||
    r.consoleErrors.length > 0 ||
    r.exceptions.length > 0 ||
    // 公开页（/models）在应用外壳之外，本来就没有侧边栏与 #main-content。
    // 对它要求 `hasMain` 是拿错尺子量 —— 那条规则是为了防"页面被踢回登录页"。
    (!PUBLIC_ROUTES.has(r.route) && !r.hasMain) ||
    !r.h1 ||
    // 被踢回登录页 = 鉴权没生效或令牌丢了，这一页根本没测到。
    r.h1.includes("无法访问") ||
    r.h1.trim() === "登录",
);

for (const row of results) {
  const flag = problems.includes(row) ? "FAIL" : "ok  ";
  console.log(
    `${flag} ${String(row.width).padStart(4)} ${row.route.padEnd(22)} overflow=${row.overflow} h1="${row.h1}"`,
  );
}

if (problems.length > 0) {
  console.log("\n--- 问题详情 ---");
  for (const row of problems) {
    console.log(`${row.width} ${row.route}`);
    if (row.overflow > 0) console.log(`  横向溢出 ${row.overflow}px`);
    if (!row.h1) console.log("  缺少 h1");
    if (!row.hasMain) console.log("  缺少 #main-content");
    for (const e of row.consoleErrors) console.log(`  console: ${e}`);
    for (const e of row.exceptions) console.log(`  exception: ${e}`);
  }
}

console.log(`\n合计 ${results.length} 项，问题 ${problems.length} 项`);
ws.close();
process.exit(problems.length > 0 ? 1 : 0);
