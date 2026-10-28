import { APIRequest } from "@/utils/request";

export type Proxy = {
  id: number;
  name: string;
  /** `socks5` / `http` / `https`。 */
  protocol: string;
  host: string;
  port: number;
  username: string;
  /** 只表示"设过密码"，不返回密码本身。 */
  hasPassword: boolean;
  /** 代理不可用时的策略：`fail` / `direct`。 */
  fallback: "fail" | "direct";
  enabled: boolean;
  note: string;
  /** 绑定到该代理的账号数。 */
  accountCount: number;
  createdAt: string;
  updatedAt: string;
};

export type ProxyPayload = {
  name: string;
  protocol: string;
  host: string;
  port: number;
  username: string;
  /** 留空 = 不改密码（编辑时）。 */
  password?: string;
  fallback: "fail" | "direct";
  enabled: boolean;
  note: string;
};

export const listProxies = () => APIRequest<Proxy[]>({ url: "/proxies", method: "GET" });

export const createProxy = (payload: ProxyPayload) =>
  APIRequest<Proxy>({ url: "/proxies", method: "POST", data: payload });

export const updateProxy = (id: number, payload: ProxyPayload) =>
  APIRequest<Proxy>({ url: `/proxies/${id}`, method: "PATCH", data: payload });

export const deleteProxy = (id: number) =>
  APIRequest<void>({ url: `/proxies/${id}`, method: "DELETE" });

/** 账号绑定/解绑代理。`proxyId = null` 表示解绑（走直连）。 */
export const setAccountProxy = (accountId: number, proxyId: number | null) =>
  APIRequest<void>({
    url: `/accounts/${accountId}/proxy`,
    method: "POST",
    data: { proxyId },
  });
