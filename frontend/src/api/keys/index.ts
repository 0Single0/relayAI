import { APIRequest } from "@/utils/request";
import type { ApiKey } from "@/api/types";

export const listKeys = () =>
  APIRequest<ApiKey[]>({ url: "/keys", method: "GET" });

export type KeyPayload = {
  name: string;
  channelAllow: string[];
  modelAllow: string[];
  rpmLimit: number | null;
  dailyTokenLimit: number | null;
  maxConcurrency: number | null;
  expiresAt: string | null;
};

/** 创建返回一次性明文：只在这一次响应里出现，之后无法再取。 */
export const createKey = (payload: KeyPayload) =>
  APIRequest<{ key: ApiKey; plaintext: string }>({
    url: "/keys",
    method: "POST",
    data: payload,
  });

export const updateKey = (id: number, payload: Partial<KeyPayload> & { enabled?: boolean }) =>
  APIRequest<ApiKey>({ url: `/keys/${id}`, method: "PATCH", data: payload });

export const deleteKey = (id: number) =>
  APIRequest<void>({ url: `/keys/${id}`, method: "DELETE" });

/** 轮换：旧 key_hash 立即失效，返回新明文。 */
export const rotateKey = (id: number) =>
  APIRequest<{ plaintext: string }>({
    url: `/keys/${id}/rotate`,
    method: "POST",
  });
