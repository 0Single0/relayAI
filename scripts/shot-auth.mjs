// 登录后截图：用于验证需要鉴权的页面。
//   node scripts/shot-auth.mjs <path> <out.png> [width] [height] [scrollTop]
import { writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const [, , path = "/dashboard", out = "shot.png", w = "1440", h = "900", scrollTop = "0"] =
  process.argv;

const ADMIN_EMAIL = "admin@relayai.local";
const ADMIN_PASSWORD = "relayai2026";

const targets = await (await fetch("http://127.0.0.1:9333/json/list")).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("没有可用的 page target（Chrome 是否带 --remote-debugging-port=9333？）");

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.addEventListener("message", (event) => {
  const msg = JSON.parse(event.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(JSON.stringify(msg.error))) : resolve(msg.result);
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
await send("Emulation.setDeviceMetricsOverride", {
  width: Number(w),
  height: Number(h),
  deviceScaleFactor: 1,
  mobile: Number(w) < 640,
});

// 先拿令牌写进 localStorage，省去每张图都走一遍表单。
const loginResponse = await fetch("http://127.0.0.1:8888/api/v1/auth/login", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ email: ADMIN_EMAIL, password: ADMIN_PASSWORD }),
});
const login = await loginResponse.json();
const token = login?.data?.token;
if (!token) {
  console.error("登录失败：", JSON.stringify(login).slice(0, 300));
  process.exit(1);
}

await send("Page.navigate", { url: "http://localhost:5173/login" });
await sleep(1500);
await send("Runtime.evaluate", {
  expression: `localStorage.setItem("relayai.auth.token", ${JSON.stringify(token)})`,
});

await send("Page.navigate", { url: `http://localhost:5173${path}` });
// 图表用"量容器 + 首帧 rAF"，比纯文本页慢一拍；等够再截，否则得到空白图。
await sleep(6000);

// 导航可能被上一张截图残留的地址覆盖（CDP 复用同一个 tab），落地后核对一次。
const landed = await send("Runtime.evaluate", {
  expression: "location.pathname",
  returnByValue: true,
});
if (landed.result.value !== path) {
  await send("Page.navigate", { url: `http://localhost:5173${path}` });
  await sleep(6000);
}

const offset = Number(scrollTop);
if (offset > 0) {
  await send("Runtime.evaluate", {
    expression: `(() => { const m = document.querySelector("main"); if (m) m.scrollTop = ${offset}; return m ? m.scrollTop : 0; })()`,
    returnByValue: true,
  });
  await sleep(800);
}

const { data } = await send("Page.captureScreenshot", { format: "png" });
writeFileSync(out, Buffer.from(data, "base64"));
console.log(`saved ${out}`);

const info = await send("Runtime.evaluate", {
  expression: `JSON.stringify({
    url: location.pathname,
    h1: document.querySelector("h1")?.innerText ?? null,
    docW: document.documentElement.scrollWidth,
    winW: window.innerWidth,
  })`,
  returnByValue: true,
});
console.log("info:", info.result.value);
ws.close();
