import { APIRequest } from "@/utils/request";

export type LiveAccountModel = {
  id: string;
  displayName: string | null;
  contextWindow: number | null;
  maxOutput: number | null;
  capabilities: { tools: boolean; vision: boolean; reasoning: boolean };
  credits: string | null;
  creditMultiplier: number | null;
  chargeType: "free" | "credits" | "unknown";
};

export type LiveAccountModels = {
  realm: "cn" | "intl";
  fetchedAtMs: number;
  models: LiveAccountModel[];
};

export const getLiveAccountModels = (accountId: number, signal?: AbortSignal) =>
  APIRequest<LiveAccountModels>({
    url: `/accounts/${accountId}/models`,
    method: "GET",
    headers: { "Cache-Control": "no-cache, no-store" },
    signal,
  });
