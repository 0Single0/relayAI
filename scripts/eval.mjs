// 用 CDP 在页面里执行一段表达式并打印结果。
//   node scripts/eval.mjs <url> "<js expression>"
const [, , url, expression] = process.argv;

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
await send("Page.navigate", { url });
await new Promise((r) => setTimeout(r, 2000));

const result = await send("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
});
console.log(JSON.stringify(result.result.value, null, 2));
if (result.exceptionDetails) {
  console.error("exception:", JSON.stringify(result.exceptionDetails, null, 2));
}
ws.close();
