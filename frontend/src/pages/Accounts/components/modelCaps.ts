import type { Account } from "@/api/types";
import type { CatalogModel } from "@/api/catalog";
import type { CustomModelDefinition } from "@/api/accounts/models";

/**
 * 账号模型相关的纯函数。
 *
 * 单独一个文件而不是放在组件里：组件文件只能导出组件，
 * 否则 Fast Refresh 会因为"既有组件又有普通函数"整块重挂。
 */

/** 未恢复的模型级限流，按恢复时刻升序（最早恢复的在前）。 */
export const activeLimits = (account: Account, now: number) =>
  (account.runtime?.modelLimits ?? [])
    .filter((item) => item.untilMs > now)
    .sort((left, right) => left.untilMs - right.untilMs);

/**
 * 展示账号支持的模型。自定义供应商直接读取账号声明，兼容旧的 ID 列表。
 * 授权平台从目录中匹配分组与账号白名单；分组发布策略不影响能力展示。
 */
export const usableModels = (
  account: Account,
  catalog: CatalogModel[],
): CatalogModel[] => {
  const allow = account.modelAllow ?? [];
  if ((account.platform || account.adapter) === "custom") {
    const declared = new Map<string, CustomModelDefinition>();
    for (const entry of account.options?.models ?? []) {
      const model = typeof entry === "string" ? { id: entry } : entry;
      declared.set(model.id, model);
    }
    const ids = allow.length ? allow : [...declared.keys()];
    return ids.map(id => {
      const definition = declared.get(id);
      const existing = catalog.find(model => model.name === id);
      return {
        name: id, displayName: definition?.display_name || id,
        contextWindow: definition?.context_length ?? existing?.contextWindow ?? null,
        maxOutput: definition?.max_output_tokens ?? existing?.maxOutput ?? null,
        capabilities: definition ? { ...definition.capabilities, tools: definition.capabilities?.tool_calls } : existing?.capabilities ?? null,
        inputPrice: existing?.inputPrice ?? 0, outputPrice: existing?.outputPrice ?? 0,
        cacheReadPrice: existing?.cacheReadPrice ?? 0, modelRatio: existing?.modelRatio ?? 1,
        enabled: true, priced: existing?.priced ?? false, channels: existing?.channels ?? [],
      };
    });
  }
  const allowSet = allow.length > 0 ? new Set(allow) : null;

  return catalog.filter((model) => {
    const exposed = model.channels.some(
      (channel) => channel.id === account.channelId,
    );
    if (!exposed) {
      return false;
    }
    return allowSet === null || allowSet.has(model.name);
  });
};

/** 上下文/输出上限的紧凑写法：1000000 → 1M。 */
export const compactTokens = (value: number | null | undefined): string => {
  if (value === null || value === undefined || value <= 0) {
    return "—";
  }
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(value % 1_000_000 === 0 ? 0 : 1)}M`;
  }
  if (value >= 1_000) {
    return `${Math.round(value / 1_000)}K`;
  }
  return String(value);
};

/** 能力徽标：只列声明为 true 的，用中文而不是字段名。 */
export const modelCapabilities = (model: Pick<CatalogModel, "capabilities">): string[] => {
  const caps = model.capabilities;
  if (!caps) {
    return [];
  }
  const list: string[] = [];
  if (caps.tools) {
    list.push("工具");
  }
  if (caps.vision) {
    list.push("视觉");
  }
  if (caps.reasoning) {
    list.push("推理");
  }
  return list;
};
