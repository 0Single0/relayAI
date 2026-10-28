import { APIRequest } from "@/utils/request";

export type PublicConfig = {
  /** 对外公布的网关根地址，如 `http://localhost:18788`。 */
  gatewayBaseUrl: string;
  siteName: string;
};

/**
 * 读取公开配置。
 *
 * **不要用 `window.location.origin` 拼客户端接入地址**：管理台跑在 5173（Vite），
 * 网关在 18788，是两个进程。用户把 5173 填进客户端会打到前端开发服务器，
 * 拿到 404 —— 看起来像"网关坏了"，其实是地址指错了。
 */
export const getPublicConfig = () =>
  APIRequest<PublicConfig>({ url: "/public-config", method: "GET" });
