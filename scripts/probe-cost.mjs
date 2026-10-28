// 悬停费用单元格的 ⓘ 图标，截图并读取浮层文本。
import { writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const targets = await (await fetch("http://127.0.0.1:9333/json/list")).json();
const page = targets.find((t) => t.type === "page");
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
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true });
  return r.result.value;
};

await new Promise((resolve) => ws.addEventListener("open", resolve, { once: true }));
await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", {
  width: 1680,
  height: 820,
  deviceScaleFactor: 1,
  mobile: false,
});

const loginResponse = await fetch("http://127.0.0.1:8888/api/v1/auth/login", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ email: "admin@relayai.local", password: "relayai2026" }),
});
const token = (await loginResponse.json())?.data?.token;
await send("Page.navigate", { url: "http://localhost:5173/login" });
await sleep(1500);
await evaluate(`localStorage.setItem("relayai.auth.token", ${JSON.stringify(token)})`);
await send("Page.navigate", { url: "http://localhost:5173/usage" });
await sleep(6500);

// 找一个已有价格快照的行（费用 > 0 且有 ⓘ）
const posText = await evaluate(`(() => {
  const spans = [...document.querySelectorAll('tbody span')];
  const target = spans.find(s => s.querySelector('svg') && /\\$0\\.\\d+/.test(s.textContent));
  if (!target) return 'none';
  const r = target.getBoundingClientRect();
  return JSON.stringify({ x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) });
})()`);
console.log("目标:", posText);

if (posText !== "none") {
  const pos = JSON.parse(posText);
  // 先移到别处再移进来，确保 mouseover 触发
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 10, y: 10, buttons: 0 });
  await sleep(200);
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pos.x, y: pos.y, buttons: 0 });
  await sleep(1000);

  const tip = await evaluate(`(() => {
    const tips = [...document.querySelectorAll('[role="tooltip"]')];
    const visible = tips.find(t => getComputedStyle(t).display !== 'none');
    return visible ? visible.innerText : 'no-visible-tooltip';
  })()`);
  console.log("---- 浮层内容 ----");
  console.log(tip);
}

const { data } = await send("Page.captureScreenshot", { format: "png" });
writeFileSync("E:\\code\\relayAI\\tmp-cost2.png", Buffer.from(data, "base64"));
console.log("saved");
ws.close();
