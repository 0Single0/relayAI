// 校验 ccswitch deeplink 的参数口径（对齐 sub2api 的 ccswitchImport.spec.ts）。
//
// 用 TypeScript 官方编译器把 `lib/ccswitch.ts` 转成 ESM 再导入，
// 不靠正则剥类型 —— 剥法一遇到 `platform?: string | null` 就会炸。
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
const require = createRequire("E:/code/relayAI/frontend/package.json");
const ts = require("typescript");

const sourcePath = "E:/code/relayAI/frontend/src/lib/ccswitch.ts";
const source = readFileSync(sourcePath, "utf8");

const compiled = ts.transpileModule(source, {
  compilerOptions: {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
  },
});

const dir = mkdtempSync(join(tmpdir(), "ccswitch-"));
const outPath = join(dir, "ccswitch.mjs");
writeFileSync(outPath, compiled.outputText, "utf8");

const { buildCcSwitchDeeplink, CC_SWITCH_USAGE_SCRIPT } = await import(
  `file://${outPath.replace(/\\/g, "/")}`
);

let pass = 0;
let fail = 0;

const check = (name, condition, detail = "") => {
  if (condition) {
    console.log(`  ok   ${name}`);
    pass += 1;
  } else {
    console.log(`  FAIL ${name} —— ${detail}`);
    fail += 1;
  }
};

const paramsOf = (link) => new URL(link.replace("ccswitch://", "https://")).searchParams;

console.log("=== ccswitch deeplink ===");

const claudeLink = buildCcSwitchDeeplink({
  baseUrl: "http://localhost:18788",
  providerName: "relayAI",
  apiKey: "sk-test-123",
  clientType: "claude",
});

check("协议前缀正确", claudeLink.startsWith("ccswitch://v1/import?"), claudeLink.slice(0, 40));

const params = paramsOf(claudeLink);
check("resource=provider", params.get("resource") === "provider", params.get("resource"));
check("app=claude", params.get("app") === "claude", params.get("app"));
check("name 为站点名", params.get("name") === "relayAI", params.get("name"));
check(
  "endpoint 不带 /v1（Claude）",
  params.get("endpoint") === "http://localhost:18788",
  params.get("endpoint"),
);
check("apiKey 原样带上", params.get("apiKey") === "sk-test-123", params.get("apiKey"));
check("configFormat=json", params.get("configFormat") === "json", params.get("configFormat"));
check("usageEnabled=true", params.get("usageEnabled") === "true", params.get("usageEnabled"));
check(
  "usageScript 可 base64 还原且含 /v1/usage",
  Buffer.from(params.get("usageScript"), "base64").toString("utf8").includes("/v1/usage"),
  "还原失败",
);
check(
  "usageAutoInterval=30",
  params.get("usageAutoInterval") === "30",
  params.get("usageAutoInterval"),
);

// openai 平台的 endpoint 必须去掉 /v1，否则 CC Switch 的 codex 会请求 /v1/v1/...
const codexParams = paramsOf(
  buildCcSwitchDeeplink({
    baseUrl: "http://localhost:18788/v1",
    platform: "openai",
    providerName: "relayAI",
    apiKey: "sk-x",
    clientType: "claude",
  }),
);
check("app=codex", codexParams.get("app") === "codex", codexParams.get("app"));
check(
  "endpoint 去掉末尾 /v1（codex）",
  codexParams.get("endpoint") === "http://localhost:18788",
  codexParams.get("endpoint"),
);

// 弹窗里显式选了 Codex CLI（没有平台信息）时也要走 codex 的 endpoint 规则。
const codexClientParams = paramsOf(
  buildCcSwitchDeeplink({
    baseUrl: "http://localhost:18788/v1",
    providerName: "relayAI",
    apiKey: "sk-x",
    clientType: "codex",
  }),
);
check("app=codex（clientType 直选）", codexClientParams.get("app") === "codex", codexClientParams.get("app"));
check(
  "endpoint 去掉末尾 /v1（clientType 直选）",
  codexClientParams.get("endpoint") === "http://localhost:18788",
  codexClientParams.get("endpoint"),
);

// grok 相反：要求带 /v1
const grokParams = paramsOf(
  buildCcSwitchDeeplink({
    baseUrl: "http://localhost:18788",
    platform: "grok",
    providerName: "relayAI",
    apiKey: "sk-x",
    clientType: "claude",
  }),
);
check("app=grokbuild", grokParams.get("app") === "grokbuild", grokParams.get("app"));
check(
  "endpoint 补上 /v1（grok）",
  grokParams.get("endpoint") === "http://localhost:18788/v1",
  grokParams.get("endpoint"),
);

check(
  "usage 脚本会去掉已有 /v1",
  CC_SWITCH_USAGE_SCRIPT.includes('/v1$/, "")'),
  "脚本里缺少去重逻辑",
);

rmSync(dir, { recursive: true, force: true });

console.log(`\n合计：通过 ${pass} 项，失败 ${fail} 项`);
if (fail > 0) {
  process.exit(1);
}
