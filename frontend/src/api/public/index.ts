import { APIRequest } from "@/utils/request";

export type PublicModel = {
  name: string;
  displayName: string;
  contextWindow: number | null;
  maxOutput: number | null;
  capabilities: Record<string, unknown> | null;
  /** 元 / 百万 token，已乘生效倍率。 */
  inputPrice: number;
  outputPrice: number;
  /** 缓存读取价；0 表示未配置（按输入价计）。 */
  cacheReadPrice: number;
  /** 是否配过价。未定价时不能显示成"免费"。 */
  priced: boolean;
};

/**
 * 公开的模型清单（无需登录）。
 *
 * 只给展示必需的信息：模型名、上下文、能力、价格。
 * **不给**渠道归属、上游模型名、账号数 —— 那些是部署细节。
 */
export const getPublicModels = () =>
  APIRequest<{ models: PublicModel[]; ratio: number }>({
    url: "/public/models",
    method: "GET",
  });
