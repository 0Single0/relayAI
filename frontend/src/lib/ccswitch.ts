/**
 * CC Switch 导入。
 *
 * **不是下载配置文件**，而是用 `ccswitch://` 协议直接把这把密钥注册进 CC Switch
 * 客户端（对齐 sub2api 的 `utils/ccswitchImport.ts`）。
 *
 * 为什么必须走 deeplink：CC Switch 的配置存在它自己的数据目录里，
 * 网页没有文件系统权限。协议处理器是它对外暴露的唯一入口 ——
 * 用户点一下，客户端自己写配置、自己建 provider，不用手工找路径、改 JSON。
 *
 * 参数口径来自 sub2api，逐项照抄（这些是 CC Switch 侧的契约，不能自己发明）：
 * | 参数 | 值 |
 * | --- | --- |
 * | `resource` | `provider` |
 * | `app` | `claude` / `codex` / `gemini` / `grokbuild` |
 * | `name` | 站点名（这里用 relayAI） |
 * | `homepage` | 网关根地址 |
 * | `endpoint` | 按平台决定是否带 `/v1` |
 * | `apiKey` | 密钥明文 |
 * | `configFormat` | `json` |
 * | `usageEnabled` / `usageScript` / `usageAutoInterval` | 余额查询脚本 |
 */

/** CC Switch 支持接入的客户端类型。 */
export type CcSwitchClientType = "claude" | "codex" | "gemini";

export type CcSwitchImportInput = {
  /** 网关根地址，如 `http://localhost:18788`。 */
  baseUrl: string;
  /** 上游协议族。relayAI 的渠道不绑平台，所以按"该渠道主要服务的协议"传。 */
  platform?: string | null;
  clientType: CcSwitchClientType;
  providerName: string;
  apiKey: string;
};

/**
 * 余额查询脚本（CC Switch 侧执行）。
 *
 * 先去掉结尾斜杠与已存在的 `/v1` 再拼 `/v1/usage`：
 * 直接拼会出现 `/v1/v1/usage` → 404 → CC Switch 显示"查询失败"。
 * 我们的 `/v1/usage` 返回 `{remaining, unit, is_active}`。
 */
export const CC_SWITCH_USAGE_SCRIPT = `({
    request: {
      url: "{{baseUrl}}".replace(/\\/+$/, "").replace(/\\/v1$/, "") + "/v1/usage",
      method: "GET",
      headers: { "Authorization": "Bearer {{apiKey}}" }
    },
    extractor: function(response) {
      const remaining = response?.remaining ?? response?.quota?.remaining ?? response?.balance;
      const unit = response?.unit ?? response?.quota?.unit ?? "USD";
      return {
        isValid: response?.is_active ?? response?.isValid ?? true,
        remaining,
        unit
      };
    }
  })`;

const withV1Endpoint = (baseUrl: string) => {
  const normalized = baseUrl.replace(/\/+$/, "");
  return normalized.endsWith("/v1") ? normalized : `${normalized}/v1`;
};

/**
 * 去掉结尾斜杠，并去掉已存在的 `/v1`。
 *
 * 名字里的 "v1" 是重点：CC Switch 的 Codex provider **自己会补**
 * OpenAI 兼容路径，我们若把 `/v1` 也传过去，客户端就会请求 `/v1/v1/...` → 404。
 * 只去斜杠是不够的（第一版就是这么写的，被 `test-ccswitch.mjs` 抓到）。
 */
const withoutV1Endpoint = (baseUrl: string) =>
  baseUrl.replace(/\/+$/, "").replace(/\/v1$/, "");

type CcSwitchConfig = {
  app: string;
  endpoint: string;
  model?: string;
};

/**
 * 按平台决定 `app` 与 `endpoint`。
 *
 * `codex` 的 endpoint **不能带 `/v1`**：CC Switch 的 Codex provider
 * 自己会补 OpenAI 兼容路径，带了就会变成 `/v1/v1/...`。
 * 这条与 `grokbuild` 相反（它要求带 `/v1`），照抄 sub2api 的实测结论。
 */
const resolveConfig = (
  platform: string | null | undefined,
  clientType: CcSwitchClientType,
  baseUrl: string,
): CcSwitchConfig => {
  switch (platform ?? "anthropic") {
    case "openai":
      return { app: "codex", endpoint: withoutV1Endpoint(baseUrl) };
    case "gemini":
      return { app: "gemini", endpoint: baseUrl };
    case "grok":
      return { app: "grokbuild", endpoint: withV1Endpoint(baseUrl) };
    default:
      // 没有平台信息时尊重用户在弹窗里显式选的客户端类型：
      // Codex 与 openai 平台走同一套 endpoint 规则（不带 /v1）。
      if (clientType === "codex") {
        return { app: "codex", endpoint: withoutV1Endpoint(baseUrl) };
      }
      return { app: clientType === "gemini" ? "gemini" : "claude", endpoint: baseUrl };
  }
};

/** 拼出 `ccswitch://v1/import?...`。 */
export const buildCcSwitchDeeplink = (input: CcSwitchImportInput): string => {
  const config = resolveConfig(input.platform, input.clientType, input.baseUrl);

  const entries: Array<[string, string]> = [
    ["resource", "provider"],
    ["app", config.app],
    ["name", input.providerName],
    ["homepage", input.baseUrl],
    ["endpoint", config.endpoint],
    ["apiKey", input.apiKey],
    ["configFormat", "json"],
    ["usageEnabled", "true"],
    // 脚本里有中文与花括号，必须 base64 才能安全放进 query。
    ["usageScript", btoa(unescape(encodeURIComponent(CC_SWITCH_USAGE_SCRIPT)))],
    ["usageAutoInterval", "30"],
  ];

  if (config.model) {
    entries.splice(2, 0, ["model", config.model]);
  }

  return `ccswitch://v1/import?${new URLSearchParams(entries).toString()}`;
};

/**
 * 触发导入。
 *
 * 返回 `false` 表示**很可能没装 CC Switch**：浏览器对未知协议是静默失败的，
 * 没有错误可捕获。sub2api 的做法是 100ms 后检查 `document.hasFocus()` ——
 * 如果页面还有焦点，说明系统没有切走，协议大概率没被处理。
 * 这个判定不是 100% 准确（有些系统会弹"是否打开"的确认框），
 * 所以只用来提示"可能没装"，不阻断任何流程。
 */
export const importToCcSwitch = (input: CcSwitchImportInput): boolean => {
  const deeplink = buildCcSwitchDeeplink(input);

  try {
    window.open(deeplink, "_self");
  } catch {
    return false;
  }

  let handled = true;
  window.setTimeout(() => {
    handled = !document.hasFocus();
  }, 100);

  return handled;
};
