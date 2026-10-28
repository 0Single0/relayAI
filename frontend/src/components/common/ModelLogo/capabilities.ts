import { Eye, GitBranch, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";

/**
 * 模型能力的中文标签与图标。
 *
 * 为什么不直接显示 `tools` / `vision` / `reasoning`：
 *   1. 那是网关的字段名，不是给人看的词；
 *   2. 一屏十几个英文单词，扫视成本比图标高得多；
 *   3. 中文 + 图标能同时表达"是什么"和"一眼归类"。
 *
 * 能力**来源**（`capabilitiesSource`）是元信息，不是能力本身，
 * 所以不在这张表里 —— 它单独用一句话表达，见 [`capabilitySourceNote`]。
 */
export type CapabilitySpec = {
  key: string;
  label: string;
  icon: LucideIcon;
  /** 悬停说明，避免"看懂了图标但不确定含义"。 */
  hint: string;
};

const SPECS: Record<string, CapabilitySpec> = {
  tools: {
    key: "tools",
    label: "工具调用",
    icon: Wrench,
    hint: "支持 function calling / tool use",
  },
  vision: {
    key: "vision",
    label: "视觉",
    icon: Eye,
    hint: "支持图片输入",
  },
  reasoning: {
    key: "reasoning",
    label: "推理",
    icon: GitBranch,
    hint: "支持思维链 / reasoning 输出",
  },
};

/**
 * 能力字段的来源。三档而不是布尔：
 *
 * · `live`     —— 上游实时目录里带的，最新也最权威；
 * · `snapshot` —— 我们按厂商官方文档维护的内置对照表，有依据但可能过时；
 * · `none`     —— 完全没有依据，调度上乐观放行，界面如实说"未知"。
 *
 * 早先把后两者压成一个 `capabilitiesKnown = false`，于是卡片上出现
 * 「工具调用 视觉 推理」与「能力未知」并列 —— 看起来像自相矛盾，
 * 实际是"有值，只是值的来源是我们自己的对照表"。
 */
export type CapabilitySource = "live" | "snapshot" | "none";

/** 按字段名取能力描述；未知字段返回 null（不显示，避免出现看不懂的英文）。 */
export const capabilityOf = (key: string): CapabilitySpec | null => SPECS[key] ?? null;

/**
 * 把 `capabilities` 对象转成**已知能力**列表。
 *
 * 只保留值为 `true` 且我们有中文标签的键：
 * 未知键硬翻成英文原文，等于把问题换个地方存在。
 */
export const knownCapabilities = (
  capabilities: Record<string, unknown> | null,
): CapabilitySpec[] => {
  if (!capabilities) {
    return [];
  }
  return Object.entries(SPECS)
    .filter(([key]) => capabilities[key] === true)
    .map(([, spec]) => spec);
};

/**
 * 读取能力来源，并**精确**兼容改版前落库的旧数据。
 *
 * `capabilitiesSource` 是后端同步目录时新写的字段。改版前只有布尔
 * `capabilitiesKnown`，但它能被还原 —— 旧网关的判断是
 * `known = !来自快照 && 有能力位`，而后端只写"为真"的键，于是：
 *
 * · `known=false` 且**有**任一能力位为真 → 只可能来自快照 → `snapshot`
 *   （实时目录带了能力位就一定是 `known=true`）；
 * · `known=false` 且**一个**能力位都没有 → 分不清"实时目录没提"还是
 *   "快照里也没有"，两者在调度上都是乐观放行，按 `none` 显示；
 * · 其余（`known` 缺省） → `live`。
 *
 * 这样在下一轮目录同步之前，老数据也不会被误标成「能力未知」。
 */
export const capabilitySource = (
  capabilities: Record<string, unknown> | null,
): CapabilitySource => {
  const raw = capabilities?.capabilitiesSource;
  if (raw === "live" || raw === "snapshot" || raw === "none") {
    return raw;
  }
  if (capabilities?.capabilitiesKnown === false) {
    const hasFlag =
      capabilities.vision === true ||
      capabilities.tools === true ||
      capabilities.reasoning === true;
    return hasFlag ? "snapshot" : "none";
  }
  return "live";
};

/**
 * 来源说明：`null` 表示不需要额外说明（上游实时声明的东西不必解释）。
 *
 * **文案必须说清"这是谁的判断"**：
 * 「能力未知」是"我们没有数据"，「按内置对照表」是"我们有数据、但来源是我们自己"。
 * 两者混为一谈时用户会以为系统坏了 —— 明明标着支持工具，却又说不知道。
 */
export const capabilitySourceNote = (
  source: CapabilitySource,
): { label: string; hint: string } | null => {
  if (source === "live") {
    return null;
  }
  if (source === "snapshot") {
    return {
      label: "按内置对照表",
      hint:
        "上游这次没有声明能力，上面这些标签来自内置对照表（按厂商官方文档维护）。" +
        "可以参考，但可能比上游当前的实际能力旧。",
    };
  }
  return {
    label: "能力未知",
    hint:
      "上游和内置对照表都没有这个模型的能力信息。调度不会据此拦请求 —— " +
      "带图片或工具调用时会直接发上去，由上游自己报错。",
  };
};
