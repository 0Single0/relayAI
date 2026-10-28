import { APIRequest } from "@/utils/request";

export type CatalogChannelRef = {
  id: string;
  name: string;
  ratio: number;
  /**
   * 该分组**对外提供**这个模型（白名单放行）。
   *
   * 上游技术上能跑不等于愿意卖：分组可以配 `model_allow` 只开放一部分。
   * 保留 `false` 的条目是为了让人看到"能跑但被白名单挡住了" ——
   * 直接过滤会让人以为上游根本不支持，排查方向就完全错了。
   */
  exposed: boolean;
};

export type CatalogModel = {
  name: string;
  displayName: string;
  contextWindow: number | null;
  maxOutput: number | null;
  capabilities: Record<string, unknown> | null;
  /** 元 / 百万 token（**未乘渠道倍率**）。 */
  inputPrice: number;
  outputPrice: number;
  cacheReadPrice: number;
  modelRatio: number;
  enabled: boolean;
  /** 是否配过价。未定价时不能显示成"免费"。 */
  priced: boolean;
  /** 该模型可用的渠道。 */
  channels: CatalogChannelRef[];
};

/** 管理面的「模型 × 渠道」清单。 */
export const listCatalogModels = () =>
  APIRequest<{ models: CatalogModel[] }>({ url: "/catalog/models", method: "GET" });

/** 从网关同步模型目录。 */
export const syncCatalog = () =>
  APIRequest<{ synced: number; removed: number; skipped: boolean }>({
    url: "/catalog/sync",
    method: "POST",
  });

/** 用内置价格表填充未定价的模型。 */
export const applyBuiltinPrices = () =>
  APIRequest<{ filled: number; usdToCny: number; message: string }>({
    url: "/models/prices/builtin",
    method: "POST",
  });

/** 从 OpenRouter 拉价格覆盖。 */
export const applyOpenRouterPrices = () =>
  APIRequest<{ updated: number; fetched: number; usdToCny: number; message: string }>({
    url: "/models/prices/openrouter",
    method: "POST",
  });

/** 手动改单个模型的价格。 */
export const updateModelPrice = (
  name: string,
  payload: {
    inputPrice?: number;
    outputPrice?: number;
    cacheReadPrice?: number;
    modelRatio?: number;
    enabled?: boolean;
  },
) =>
  APIRequest<unknown>({
    url: `/models/${encodeURIComponent(name)}`,
    method: "PATCH",
    data: payload,
  });
