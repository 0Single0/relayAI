import type { Account, Channel, ChannelAdapter, ChannelOptions } from "@/api/types";

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

/** 可选的平台清单（新建账号时用）。
 *
 * **平台是具体平台，不含"国内/国际"这种可选项。**
 * 国内版与国际版打的是不同上游域名（`copilot.tencent.com` vs `workbuddy.ai`），
 * 凭据也只对其中一个站有效 —— 它们是两个平台，不是一个平台的两个选项。
 *
 * 把版本合并进平台名（`workbuddy-cn`）而不再单列一个"区域"下拉：
 *   · 之前"WorkBuddy + 区域"两个字段，用户很容易选了国际版却忘了改区域；
 *   · 分组也只能放同平台的号（不做 composite），所以一个分组天然对应一个版本。
 */
export const platformOptions: Array<{
  /** 平台标识（落库到 `accounts.platform`）。 */
  value: string;
  label: string;
  hint: string;
  /** 是否走 OAuth 授权登录（而不是填 API Key）。 */
  oauth: boolean;
  /** WorkBuddy 的区域；其它平台为空。 */
  realm?: string;
}> = [
  {
    value: "workbuddy-cn",
    label: "WorkBuddy 国内版",
    hint: "copilot.tencent.com，有成长任务与签到",
    oauth: true,
    realm: "cn",
  },
  {
    value: "workbuddy-intl",
    label: "WorkBuddy 国际版",
    hint: "workbuddy.ai，只有每日活跃打卡",
    oauth: true,
    realm: "intl",
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

/**
 * 把具体平台还原成 适配器 + 区域。
 *
 * 网关与后端按 `adapter`（workbuddy / trae / custom）+ `options.realm` 工作；
 * 界面用具体平台。这里是两者之间的唯一转换点 ——
 * 转换逻辑集中在此，避免各处各写一遍 if。
 */
export const platformParts = (
  platform: string,
): { adapter: string; realm?: string } => {
  const found = platformOptions.find((item) => item.value === platform);
  if (found) {
    return found.realm
      ? { adapter: found.value.split("-")[0], realm: found.realm }
      : { adapter: found.value };
  }
  // 兼容老数据：库里可能还是裸 `workbuddy`。
  return { adapter: platform };
};

/** 反查：适配器 + 区域 → 具体平台。 */
export const platformFromParts = (adapter: string, realm?: string): string => {
  if (adapter === "workbuddy") {
    return realm === "intl" ? "workbuddy-intl" : "workbuddy-cn";
  }
  return adapter;
};

/** 账号的平台名称包含具体版本，与创建及分组兼容性判断使用同一份映射。 */
export const accountPlatformLabel = (account: Pick<Account, "platform" | "adapter" | "options">) => {
  const platform = platformFromParts(account.platform || account.adapter, account.options?.realm);
  return platformOptions.find((option) => option.value === platform)?.label ?? platformLabel(platform);
};

export const outboundOptions = [
  { value: "desktop", label: "桌面端" },
  { value: "cli", label: "官方 CLI" },
  { value: "vscode", label: "VSCode 插件" },
];

export const accountMatchesChannel = (account: Pick<Account, "platform" | "adapter" | "options">, channel: Channel) =>
  platformFromParts(account.platform || account.adapter, account.options?.realm) ===
  platformFromParts(channel.adapter, channel.options?.realm);

export const upstreamProtocolOptions = [
  { value: "openai-chat", label: "OpenAI Chat Completions" },
  { value: "openai-responses", label: "OpenAI Responses" },
  { value: "anthropic", label: "Anthropic Messages" },
];

/** 该平台是否用账号登录态（影响表单字段与提示）。 */
export const isAccountPlatform = (platform: string) =>
  platform.startsWith("workbuddy") || platform === "trae";

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
