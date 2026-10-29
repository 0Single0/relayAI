import { APIRequest } from "@/utils/request";
import type { Account, Channel, PageResult } from "@/api/types";
import type { CustomModelDefinition } from "./models";

export type AccountQuery = {
  channelId?: string;
  status?: string;
  keyword?: string;
  page?: number;
  pageSize?: number;
};

export const listAccounts = (query: AccountQuery = {}) =>
  APIRequest<PageResult<Account>>({
    url: "/accounts",
    method: "GET",
    params: query,
  });

export const getAccount = (id: number) =>
  APIRequest<Account>({ url: `/accounts/${id}`, method: "GET" });

export type AccountPayload = {
  nickname?: string;
  enabled?: boolean;
  priorityOverride?: number | null;
  weightOverride?: number | null;
  maxInflight?: number | null;
  baseUrl?: string;
  protocol?: string;
  apiKey?: string;
  options?: Account["options"];
  modelAllow?: string[];
  modelRename?: Record<string, string>;
  /**
   * 迁移到另一个分组。
   *
   * 只改 `channel_id`，uid 与凭据都不动 —— 所以历史用量仍然对得上。
   * 不传表示不迁移（与"传空串"不同：空串会被后端拒绝）。
   */
  channelId?: string;
};

/** 手动清掉该账号的冷却（账号级 + 模型级）。 */
export const resetAccountCooldowns = (id: number) =>
  APIRequest<{ cleared: boolean }>({ url: `/accounts/${id}/reset`, method: "POST" });

export const updateAccount = (id: number, payload: AccountPayload) =>
  APIRequest<Account>({ url: `/accounts/${id}`, method: "PATCH", data: payload });

export const deleteAccount = (id: number) =>
  APIRequest<void>({ url: `/accounts/${id}`, method: "DELETE" });

/**
 * 新建账号（静态凭据）。
 *
 * `platform` 必填：平台是**账号**属性，决定用哪个适配器。
 * `baseUrl` / `protocol` 也是账号级 —— 一把 Key 属于哪家上游。
 */
export const createCredentialAccount = (payload: {
  channelId: string;
  uid: string;
  nickname: string;
  apiKey: string;
  platform: string;
  baseUrl?: string;
  protocol?: string;
  options?: Record<string, unknown>;
  modelAllow?: string[];
}) => APIRequest<Account>({ url: "/accounts", method: "POST", data: payload });

export const discoverModels = (payload: { baseUrl: string; apiKey: string; protocol: string; accountId?: number }) =>
  APIRequest<CustomModelDefinition[]>({ url: "/model-discoveries", method: "POST", data: payload });

/**
 * 登录流程按**渠道 + 平台**发起，不是按账号。
 *
 * 新增账号时账号还不存在，所以登录三件套挂在渠道上，成功后由后端建号。
 * 平台必须一起传：它决定用哪个适配器登录（WorkBuddy 与 Trae 的授权流程完全不同），
 * 也决定新账号的 `platform` 字段写成什么。
 * `state` 是上游票据，轮询与回调都必须带上它。
 */
export type LoginTicket = {
  state: string;
  authUrl: string;
  /** 是否需要在授权后把回调地址粘回来（Trae 这类回环回调）。 */
  needsCallback: boolean;
  /** 该平台是否有真正的授权页；false 表示只能手填凭据。 */
  hasAuthPage: boolean;
  hint: string;
};

export const startLogin = (channelId: string, platform: string, realm?: string) =>
  APIRequest<LoginTicket>({
    url: `/channels/${encodeURIComponent(channelId || "__unassigned__")}/login/start`,
    method: "POST",
    data: { platform, realm },
  });

export type LoginPollResult = {
  state: "pending" | "ready" | "failed";
  message: string;
  account?: Account;
};

export const pollLogin = (channelId: string, state: string, platform?: string, options?: Record<string, unknown>) =>
  APIRequest<LoginPollResult>({
    url: `/channels/${encodeURIComponent(channelId || "__unassigned__")}/login/poll`,
    method: "POST",
    data: { state, platform, options },
  });

export const completeLogin = (
  channelId: string,
  state: string,
  callback: string,
  platform?: string,
  options?: Record<string, unknown>,
) =>
  APIRequest<LoginPollResult>({
    url: `/channels/${encodeURIComponent(channelId || "__unassigned__")}/login/complete`,
    method: "POST",
    data: { state, callback, platform, options },
  });

export const refreshCredential = (id: number) =>
  APIRequest<{ ok: boolean; expiresAt: string | null; message: string }>({
    url: `/accounts/${id}/refresh`,
    method: "POST",
  });

/**
 * 测试凭据。
 *
 * `model` 非空时只测那一个模型 —— 上游限流按模型生效，
 * 指定模型才能区分"这个号废了"和"这个模型在等"。
 */
export const probeAccount = (id: number, model?: string) =>
  APIRequest<{ ok: boolean; latencyMs: number; reply: string; error: string | null }>({
    url: model
      ? `/accounts/${id}/probe?model=${encodeURIComponent(model)}`
      : `/accounts/${id}/probe`,
    method: "POST",
  });

export type QuotaSnapshot = {
  label: string;
  unit: string;
  remaining: number | null;
  total: number | null;
  used: number | null;
  details: Array<[string, string]>;
  fetchedAt: string;
};

export const getAccountQuota = (id: number) =>
  APIRequest<QuotaSnapshot | null>({ url: `/accounts/${id}/quota`, method: "GET" });

/** 单个账号的积分刷新结果。 */
export type QuotaRefreshResult = {
  accountId: number;
  uid: string;
  ok: boolean;
  /** 失败原因；成功时缺省。 */
  error?: string;
};

/** 批量刷新积分的汇总。 */
export type QuotaRefreshSummary = {
  total: number;
  ok: number;
  results: QuotaRefreshResult[];
};

/**
 * 批量刷新积分：强制对每个账号真实打一次上游的额度接口。
 *
 * 与 `listAccounts` 的区别：后者读网关内存快照（巡检 5 分钟才刷一次），
 * 这个立即生效 —— 「刷新积分」按钮与 30s 自动轮询走这里。
 * `ids` 缺省 = 刷新全部账号。
 */
export const refreshAccountQuotas = (ids?: number[]) =>
  APIRequest<QuotaRefreshSummary>({
    url: "/accounts/refresh-quotas",
    method: "POST",
    data: { ids },
  });

/** 任务组二级菜单的数据源（SPEC 8.3）。 */
export const listTaskChannels = () =>
  APIRequest<import("@/api/types").TaskChannel[]>({
    url: "/tasks/channels",
    method: "GET",
  });

export type { Account, Channel };
