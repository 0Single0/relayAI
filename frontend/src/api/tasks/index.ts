import { APIRequest } from "@/utils/request";
import type {
  PageResult,
  TaskChannel,
  TaskChannelLive,
  TaskRun,
  TaskRunItem,
  TaskRunStatus,
  TaskScope,
} from "@/api/types";

export const listTaskChannels = () =>
  APIRequest<TaskChannel[]>({ url: "/tasks/channels", method: "GET" });

/**
 * 任务快照。
 *
 * `channelId` 传**版本**（`cn` / `intl`）：后端按版本跨分组取号，
 * 未分配分组的号也会包含在内。传具体分组 id 仍可用（兼容），
 * 但只返回该分组下的号。
 */
export const getTaskChannelLive = (channelId: string, signal?: AbortSignal) =>
  APIRequest<TaskChannelLive>({
    url: `/tasks/channels/${encodeURIComponent(channelId)}/live`,
    method: "GET",
    signal,
    headers: { "Cache-Control": "no-cache", Pragma: "no-cache" },
  });

export type StartRunPayload = {
  /**
   * 目标账号（UID + 各自所属分组）。
   *
   * 必须逐账号带分组：任务页按版本取号，同屏可能有多个分组、
   * 也可能有还没分配分组的号（`channelId` 为空串）。
   */
  accounts: Array<{ uid: string; channelId: string }>;
  scope: TaskScope;
  /**
   * 只执行这些任务码；为空则按 `scope` 跑整批。
   *
   * 行内「执行此任务」必须传它 —— 不传就等于整批执行。
   */
  codes?: string[];
};

export const startTaskRun = (payload: StartRunPayload) =>
  APIRequest<{ batchId: string; runs: Array<{ runId: number; accountUid: string }> }>({
    url: "/tasks/runs",
    method: "POST",
    data: payload,
  });

export const cancelTaskRun = (runId: number) =>
  APIRequest<void>({ url: `/tasks/runs/${runId}/cancel`, method: "POST" });

export type RunQuery = {
  channel?: string;
  account?: string;
  status?: TaskRunStatus;
  page?: number;
  pageSize?: number;
};

export const listTaskRuns = (query: RunQuery = {}) =>
  APIRequest<PageResult<TaskRun>>({
    url: "/tasks/runs",
    method: "GET",
    params: query,
  });

export const getTaskRun = (runId: number) =>
  APIRequest<{ run: TaskRun; items: TaskRunItem[] }>({
    url: `/tasks/runs/${runId}`,
    method: "GET",
  });

/**
 * 任务进度的 SSE 地址（SPEC 9.8「行内实时结果通过 SSE 更新」）。
 *
 * 不走 `APIRequest`：`EventSource` 不算 axios 请求，它自己管连接与重连。
 * 路由必须是同源相对路径 —— `EventSource` 不支持自定义请求头，
 * 也不能跨域，所以这里直接拼 `VITE_BASE_URL`（默认 `/api/v1`）。
 *
 * `after` 是断点：浏览器重连时把已收到的最后一条明细 id 带上来，
 * 服务端只补之后的，不会把整批结果重放一遍。
 */
export const taskStreamUrl = (runId: number, after = 0) => {
  const base = import.meta.env.VITE_BASE_URL || "/api/v1";
  return `${base}/tasks/runs/${runId}/stream?after=${after}`;
};

/** SSE 推进里的一帧进度（与后端 `TaskRunProgress` 对应）。 */
export type TaskStreamProgress = {
  id: number;
  status: TaskRunStatus;
  total: number;
  doneCount: number;
  failedCount: number;
  skippedCount: number;
  earned: number;
  error: string | null;
};

export const claimTask = (channelId: string, accountUid: string, code: string) =>
  APIRequest<TaskRunItem>({
    url: "/tasks/claim",
    method: "POST",
    data: { channelId, accountUid, code },
  });
