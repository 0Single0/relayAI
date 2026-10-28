// 用 CDP 对当前页面截图。用法：
//   node scripts/shoot.mjs <url> <out.png> [width] [height] [fullPage] [scrollTop]
// 依赖：本机 Chrome 以 --remote-debugging-port=9333 启动。
//
// `scrollTop`：本应用的外壳是"只让内容区内部滚动"（main 上有 overflow-y），
// 整页并不滚动，所以 captureBeyondViewport 抓不到折线以下的内容 ——
// 必须直接滚内部容器再截图。
import { writeFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const [, , url, out, w = "1440", h = "900", full = "false", scrollTop = "0"] =
  process.argv;
const width = Number(w);
const height = Number(h);

const targets = await (await fetch("http://127.0.0.1:9333/json/list")).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("没有可用的 page target");

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
  width,
  height,
  deviceScaleFactor: 1,
  mobile: width < 640,
});
await send("Page.navigate", { url });
await sleep(2200);

const offset = Number(scrollTop);
if (offset > 0) {
  await send("Runtime.evaluate", {
    expression: `(() => {
      const main = document.querySelector("main");
      if (main) main.scrollTop = ${offset};
      window.scrollTo(0, ${offset});
      return main ? main.scrollTop : 0;
    })()`,
    returnByValue: true,
  });
  await sleep(700);
}

const { data } = await send("Page.captureScreenshot", {
  format: "png",
  captureBeyondViewport: full === "true",
});
writeFileSync(out, Buffer.from(data, "base64"));
console.log(`saved ${out}`);

const exceptions = await send("Runtime.evaluate", {
  expression: "JSON.stringify(window.__errors ?? [])",
  returnByValue: true,
});
console.log("page errors:", exceptions.result.value);

ws.close();
