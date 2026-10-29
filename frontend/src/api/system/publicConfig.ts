import { APIRequest } from "@/utils/request";

export type PublicConfig = {
  /** 对外公布的网关根地址，如 `http://localhost:18788`。 */
  gatewayBaseUrl: string;
  /** 仅当网关与管理台共用入口时，由后端显式启用。 */
  gatewaySameOrigin?: boolean;
  siteName: string;
};

/**
 * 读取公开配置。
 *
 * Docker 共用入口时按当前访问地址解析；独立网关保持后端配置的地址。
 * 不在请求失败或缺少配置时猜测地址。
 */
export const getPublicConfig = async () => {
  const response = await APIRequest<PublicConfig>({ url: "/public-config", method: "GET" });
  if (response.data?.gatewaySameOrigin) {
    response.data = { ...response.data, gatewayBaseUrl: window.location.origin };
  }
  return response;
};
