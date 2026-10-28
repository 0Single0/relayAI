import { APIRequest } from "@/utils/request";
import type { GatewayStatus, Settings } from "@/api/types";

export const getGatewayStatus = () =>
  APIRequest<GatewayStatus>({ url: "/status", method: "GET" });

export type DecisionEntry = {
  trace: string;
  ts: string;
  model: string;
  stream: boolean;
  session: string | null;
  candidates: number;
  rejected: string[];
  attempts: number;
  viaAffinity: boolean;
  channel: string | null;
  account: string | null;
  upstreamModel: string | null;
  error: { kind: string; message: string } | null;
};

export const getRecentDecisions = (limit = 50) =>
  APIRequest<DecisionEntry[]>({
    url: "/status/decisions",
    method: "GET",
    params: { limit },
  });

export const getSettings = () =>
  APIRequest<Settings>({ url: "/settings", method: "GET" });

export const updateSettings = (payload: Partial<Settings>) =>
  APIRequest<Settings>({ url: "/settings", method: "PATCH", data: payload });
