import { APIRequest } from "@/utils/request";
import type { Account, Channel, PageResult } from "@/api/types";

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
  modelAllow?: string[];
  modelRename?: Record<string, string>;
};

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
}) => APIRequest<Account>({ url: "/accounts", method: "POST", data: payload });

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
    url: `/channels/${encodeURIComponent(channelId)}/login/start`,
    method: "POST",
    data: { platform, realm },
  });

export type LoginPollResult = {
  state: "pending" | "ready" | "failed";
  message: string;
  account?: Account;
};

export const pollLogin = (channelId: string, state: string, platform?: string) =>
  APIRequest<LoginPollResult>({
    url: `/channels/${encodeURIComponent(channelId)}/login/poll`,
    method: "POST",
    data: { state, platform },
  });

export const completeLogin = (
  channelId: string,
  state: string,
  callback: string,
  platform?: string,
) =>
  APIRequest<LoginPollResult>({
    url: `/channels/${encodeURIComponent(channelId)}/login/complete`,
    method: "POST",
    data: { state, callback, platform },
  });

export const refreshCredential = (id: number) =>
  APIRequest<{ ok: boolean; expiresAt: string | null; message: string }>({
    url: `/accounts/${id}/refresh`,
    method: "POST",
  });

export const probeAccount = (id: number) =>
  APIRequest<{ ok: boolean; latencyMs: number; reply: string; error: string | null }>({
    url: `/accounts/${id}/probe`,
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

/** 任务组二级菜单的数据源（SPEC 8.3）。 */
export const listTaskChannels = () =>
  APIRequest<import("@/api/types").TaskChannel[]>({
    url: "/tasks/channels",
    method: "GET",
  });

export type { Account, Channel };
