import { APIRequest } from "@/utils/request";
import type {
  DashboardOverview,
  PageResult,
  SummaryBucket,
  UsageLog,
  UsageSeriesPoint,
  UsageStatus,
  UsageSummary,
} from "@/api/types";

export type UsageQuery = {
  from?: string;
  to?: string;
  channelId?: string;
  accountUid?: string;
  keyId?: number;
  model?: string;
  status?: UsageStatus;
  page?: number;
  pageSize?: number;
};

export const listUsage = (query: UsageQuery = {}) =>
  APIRequest<PageResult<UsageLog>>({
    url: "/usage",
    method: "GET",
    params: query,
  });

export const getUsage = (id: number) =>
  APIRequest<UsageLog>({ url: `/usage/${id}`, method: "GET" });

export type SummaryGroup = "channel" | "model" | "key" | "day";

export type { SummaryBucket, UsageSummary };

export const getUsageSummary = (query: {
  from?: string;
  to?: string;
  groupBy?: SummaryGroup;
}) =>
  APIRequest<{ totals: UsageSummary; buckets: SummaryBucket[] }>({
    url: "/usage/summary",
    method: "GET",
    params: query,
  });

export const getUsageSeries = (query: {
  from?: string;
  to?: string;
  model?: string;
  bucket?: "hour" | "day";
}) =>
  APIRequest<UsageSeriesPoint[]>({
    url: "/usage/series",
    method: "GET",
    params: query,
  });

/**
 * 仪表盘快照。
 *
 * `range` 只影响趋势图的横轴范围；"今日"与"累计"由后端按自然日 /
 * 全量各自计算，不受它影响。
 */
export const getDashboardOverview = (query: {
  from?: string;
  to?: string;
  bucket?: "hour" | "day";
}) =>
  APIRequest<DashboardOverview>({
    url: "/dashboard",
    method: "GET",
    params: query,
  });
