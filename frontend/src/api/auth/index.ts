import { APIRequest } from "@/utils/request";
import type { PageResult } from "@/api/types";

/** 管理台用户。 */
export type User = {
  id: number;
  email: string;
  nickname: string;
  balance: number;
  totalRecharged: number;
  role: "admin" | "user";
  enabled: boolean;
  source: string;
  createdAt: string;
  lastLoginAt: string | null;
};

export type AuthPayload = {
  token: string;
  user: User;
};

export const register = (payload: {
  email: string;
  password: string;
  nickname?: string;
}) => APIRequest<AuthPayload>({ url: "/auth/register", method: "POST", data: payload });

export const login = (payload: { email: string; password: string }) =>
  APIRequest<AuthPayload>({ url: "/auth/login", method: "POST", data: payload });

export const logout = () => APIRequest<void>({ url: "/auth/logout", method: "POST" });

export const getMe = () => APIRequest<User>({ url: "/auth/me", method: "GET" });

export const redeem = (code: string) =>
  APIRequest<{ user: User; amount: number; msg: string }>({
    url: "/auth/redeem",
    method: "POST",
    data: { code },
  });

export type BalanceTransaction = {
  id: number;
  amount: number;
  balanceAfter: number;
  kind: "redeem" | "usage" | "adjust";
  note: string;
  createdAt: string;
};

export const listTransactions = (query: { page?: number; pageSize?: number } = {}) =>
  APIRequest<PageResult<BalanceTransaction>>({
    url: "/auth/transactions",
    method: "GET",
    params: query,
  });

// ---- 管理端：用户与兑换码 ----

export const listUsers = (query: { page?: number; pageSize?: number } = {}) =>
  APIRequest<PageResult<User>>({ url: "/users", method: "GET", params: query });

/** 管理员直接建用户（可指定角色与初始余额）。 */
export const createUser = (payload: {
  email: string;
  password: string;
  nickname?: string;
  role?: "admin" | "user";
  balance?: number;
}) => APIRequest<User>({ url: "/users", method: "POST", data: payload });

/** 管理员调整余额：正数加、负数减。 */
export const adjustBalance = (userId: number, amount: number, note: string) =>
  APIRequest<User>({
    url: `/users/${userId}/balance`,
    method: "POST",
    data: { amount, note },
  });

/** 管理员编辑用户（只提交改动的字段）。 */
export const updateUser = (
  userId: number,
  payload: { nickname?: string; role?: "admin" | "user"; enabled?: boolean },
) => APIRequest<User>({ url: `/users/${userId}`, method: "PATCH", data: payload });

/** 管理员重置密码（同时吊销该用户所有会话）。 */
export const resetPassword = (userId: number, password: string) =>
  APIRequest<void>({
    url: `/users/${userId}/password`,
    method: "POST",
    data: { password },
  });

export type RedeemCode = {
  id: number;
  code: string;
  amount: number;
  note: string;
  expiresAt: string | null;
  usedBy: number | null;
  usedAt: string | null;
  createdAt: string;
};

export const listRedeemCodes = (
  query: { page?: number; pageSize?: number } = {},
) =>
  APIRequest<PageResult<RedeemCode>>({
    url: "/redeem-codes",
    method: "GET",
    params: query,
  });

export const createRedeemCodes = (payload: {
  amount: number;
  count: number;
  note?: string;
  expiresAt?: string;
}) => APIRequest<RedeemCode[]>({ url: "/redeem-codes", method: "POST", data: payload });
