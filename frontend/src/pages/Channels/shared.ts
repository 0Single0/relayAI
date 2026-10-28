import type { ChannelAdapter, ChannelOptions } from "@/api/types";

/**
 * 平台（上游适配器）的中文名。
 *
 * 注意措辞：这里是**平台**，不是渠道。
 * 渠道是计价分组（倍率 / 优先级），平台是"接的是哪家上游"。
 */
export const platformLabel = (platform: string) => {
  switch (platform) {
    case "workbuddy":
      return "WorkBuddy";
    case "trae":
      return "Trae";
    case "custom":
      return "自定义供应商";
    case "fake":
      return "链路自测";
    default:
      return platform;
  }
};

/** 兼容旧名字：渠道列表里显示的也是平台名。 */
export const adapterLabel = (adapter: ChannelAdapter) => platformLabel(adapter);

/** 可选的平台清单（新建账号时用）。 */
export const platformOptions: Array<{
  value: string;
  label: string;
  hint: string;
  /** 是否走 OAuth 授权登录（而不是填 API Key）。 */
  oauth: boolean;
}> = [
  {
    value: "workbuddy",
    label: "WorkBuddy",
    hint: "国内版 / 国际版，走授权登录",
    oauth: true,
  },
  {
    value: "trae",
    label: "Trae",
    hint: "国内版，走授权登录（需粘贴回调地址）",
    oauth: true,
  },
  {
    value: "custom",
    label: "自定义供应商",
    hint: "任意 OpenAI / Anthropic 兼容上游，填 Base URL + API Key",
    oauth: false,
  },
];

export const realmOptions = [
  { value: "cn", label: "国内版" },
  { value: "intl", label: "国际版" },
];

export const outboundOptions = [
  { value: "desktop", label: "桌面端" },
  { value: "cli", label: "官方 CLI" },
  { value: "vscode", label: "VSCode 插件" },
];

export const upstreamProtocolOptions = [
  { value: "openai-chat", label: "OpenAI Chat Completions" },
  { value: "openai-responses", label: "OpenAI Responses" },
  { value: "anthropic", label: "Anthropic Messages" },
];

/** 该平台是否用账号登录态（影响表单字段与提示）。 */
export const isAccountPlatform = (platform: string) =>
  platform === "workbuddy" || platform === "trae";

/** 平台能力的默认值（新建时的初始状态，实际以网关探测为准）。 */
export const defaultCaps = (platform: string, options: ChannelOptions) => {
  if (platform === "workbuddy") {
    const isCn = (options.realm ?? "cn") === "cn";
    return { tasks: isCn, daily: true, checkin: true };
  }
  if (platform === "trae") {
    return { tasks: false, daily: false, checkin: true };
  }
  return { tasks: false, daily: false, checkin: false };
};
