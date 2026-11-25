import { APIRequest } from "@/utils/request";
import type { Channel, ChannelHealth, Model, OpsCaps } from "@/api/types";

export const listChannels = () =>
  APIRequest<Channel[]>({ url: "/channels", method: "GET" });

export const getChannel = (id: string) =>
  APIRequest<Channel>({ url: `/channels/${encodeURIComponent(id)}`, method: "GET" });

/** 渠道健康度卡片数据。`days` 是统计窗口。 */
export const listChannelHealth = (days = 7) =>
  APIRequest<ChannelHealth[]>({
    url: "/channels/health",
    method: "GET",
    params: { days },
  });

export type ChannelPayload = {
  id: string;
  name: string;
  modelProvider: string;
  adapter: Channel["adapter"];
  enabled: boolean;
  priority: number;
  weight: number;
  options: Channel["options"];
  channelRatio: number;
  concurrencyLimit: number | null;
  rateLimitQps: number | null;
  /** 对外模型发布规则（精确名称 / * 通配符）；空数组 = 清空限制。 */
  modelAllow?: string[];
  /** 旧版单值探活模型；空串 = 清空。 */
  probeModel?: string;
  /**
   * 定时探活**真正读取**的模型列表。
   *
   * 与 `probeModel` 一起写：只改显示字段的话，探活仍按旧列表跑。
   */
  probeModels?: string[];
  /**
   * 是否在「分组状态」页展示这个分组。
   *
   * 不传 = 不改；新建时不传等于"不展示"（默认值）。
   */
  statusVisible?: boolean;
};

export const createChannel = (payload: ChannelPayload) =>
  APIRequest<Channel>({ url: "/channels", method: "POST", data: payload });

export const updateChannel = (id: string, payload: Partial<ChannelPayload>) =>
  APIRequest<Channel>({
    url: `/channels/${encodeURIComponent(id)}`,
    method: "PATCH",
    data: payload,
  });

export const deleteChannel = (id: string) =>
  APIRequest<void>({ url: `/channels/${encodeURIComponent(id)}`, method: "DELETE" });

export type ProbeResult = {
  ok: boolean;
  model: string;
  latencyMs: number;
  reply: string;
  error: string | null;
};

export const probeChannel = (id: string) =>
  APIRequest<ProbeResult>({
    url: `/channels/${encodeURIComponent(id)}/probe`,
    method: "POST",
  });

export type ChannelModelEntry = {
  id: string;
  upstreamId: string;
  contextWindow: number | null;
  maxOutput: number | null;
  caps: OpsCaps & { vision?: boolean; toolCalls?: boolean; reasoning?: boolean };
  source: "live" | "builtin";
};

export const listChannelModels = (id: string) =>
  APIRequest<ChannelModelEntry[]>({
    url: `/channels/${encodeURIComponent(id)}/models`,
    method: "GET",
  });

export const listModels = () =>
  APIRequest<Model[]>({ url: "/models", method: "GET" });
