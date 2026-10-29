import type { CustomModelDefinition } from "@/api/accounts/models";

export type ReasoningLevel = { key: string; level: string; value: string };
export type CustomModelDraft = {
  key: string;
  id: string;
  name: string;
  context: string;
  output: string;
  vision: boolean;
  tools?: boolean;
  reasoning: boolean;
  levels: ReasoningLevel[];
  defaultEffort?: string;
};

export const reasoningLevels = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
export const defaultLevels = (): ReasoningLevel[] => ["off", "low", "medium", "high"].map(level => ({ key: crypto.randomUUID(), level, value: level === "off" ? "none" : level }));
export const parseTokenLimit = (value: string): number | undefined => {
  if (!value.trim()) return undefined;
  const match = /^(\d+(?:\.\d+)?)\s*([kKmM]?)$/.exec(value.trim());
  if (!match) return NaN;
  const result = Number(match[1]) * ({ k: 1000, m: 1000000 }[match[2].toLowerCase()] ?? 1);
  return Number.isSafeInteger(result) && result > 0 ? result : NaN;
};
const compactTokens = (value?: number | null) => value ? value % 1000000 === 0 ? `${value / 1000000}M` : value % 1000 === 0 ? `${value / 1000}K` : String(value) : "";

export const createModelDraft = (model?: CustomModelDefinition): CustomModelDraft => ({
  key: crypto.randomUUID(), id: model?.id ?? "", name: model?.display_name ?? model?.id ?? "",
  context: compactTokens(model?.context_length), output: compactTokens(model?.max_output_tokens),
  vision: model?.capabilities?.vision ?? false,
  tools: model?.capabilities?.declared_tools === false ? undefined : model?.capabilities?.tool_calls,
  reasoning: model?.capabilities?.reasoning ?? Boolean(model?.reasoning_efforts?.length),
  levels: model?.reasoning_efforts?.length ? model.reasoning_efforts.map(level => ({ key: crypto.randomUUID(), level: level === "none" ? "off" : level, value: model.reasoning_effort_map?.[level] ?? level })) : defaultLevels(),
  defaultEffort: model?.reasoning_default_effort ?? undefined,
});

export const getCustomModelError = (models: CustomModelDraft[]): string => {
  if (!models.length) return "请至少添加一个支持的模型";
  const ids = new Set<string>();
  for (const [index, model] of models.entries()) {
    const prefix = `第 ${index + 1} 个模型：`;
    if (!model.id.trim() || /\s/.test(model.id.trim())) return `${prefix}请填写不含空格的模型 ID`;
    if (ids.has(model.id.trim())) return `${prefix}模型 ID 重复`;
    ids.add(model.id.trim());
    const context = parseTokenLimit(model.context), output = parseTokenLimit(model.output);
    if (Number.isNaN(context) || Number.isNaN(output)) return `${prefix}token 数应为正整数，支持 32K、1M 等写法`;
    if (context && output && output > context) return `${prefix}最大输出不能超过上下文窗口`;
    if (model.reasoning) {
      if (!model.levels.length) return `${prefix}请至少配置一个推理等级`;
      const levels = new Set<string>();
      for (const item of model.levels) {
        if (!item.level.trim() || !item.value.trim()) return `${prefix}推理等级和 API 参数值不能为空`;
        if (levels.has(item.level)) return `${prefix}推理等级重复`;
        levels.add(item.level);
      }
    }
  }
  return "";
};

export const serializeCustomModels = (models: CustomModelDraft[]): CustomModelDefinition[] => models.map(model => ({
  id: model.id.trim(), display_name: model.name.trim() || model.id.trim(),
  context_length: parseTokenLimit(model.context), max_output_tokens: parseTokenLimit(model.output),
  capabilities: { vision: model.vision, ...(model.tools === undefined ? {} : { tool_calls: model.tools }), reasoning: model.reasoning },
  reasoning_efforts: model.reasoning ? model.levels.map(item => item.level) : [],
  reasoning_effort_map: model.reasoning ? Object.fromEntries(model.levels.map(item => [item.level, item.value.trim()])) : {},
  ...(model.reasoning && model.defaultEffort && model.levels.some(item => item.level === model.defaultEffort) ? { reasoning_default_effort: model.defaultEffort } : {}),
}));
