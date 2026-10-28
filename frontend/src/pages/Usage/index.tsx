import { Download, FileClock, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { listChannels } from "@/api/channels";
import { listUsage } from "@/api/usage";
import type { Channel, UsageLog, UsageStatus } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { DatePicker, type DateRange } from "@/components/common/DatePicker";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Pagination } from "@/components/common/Pagination";
import { StatusBadge } from "@/components/common/StatusBadge";
import { CostBreakdown } from "@/pages/Usage/components/CostBreakdown";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const ALL = "__all__";
const PAGE_SIZE = 50;

const statusTone = (status: UsageStatus) => {
  switch (status) {
    case "completed":
      return "success" as const;
    case "rejected":
      return "neutral" as const;
    case "client_aborted":
      return "warning" as const;
    default:
      return "danger" as const;
  }
};

const statusLabel = (status: UsageStatus) => {
  switch (status) {
    case "completed":
      return "成功";
    case "failed":
      return "失败";
    case "client_aborted":
      return "客户端中断";
    case "upstream_aborted":
      return "上游断流";
    case "rejected":
      return "未调度";
    default:
      return status;
  }
};

/**
 * 协议的中文标签。
 *
 * 显示**入站端点**而不是协议代号：用户关心的是"我从哪个接口打进来的"，
 * `/v1/chat/completions` 比 `chat` 更能直接对上他代码里的 URL。
 */
const protocolLabel = (protocol: UsageLog["protocol"]) => {
  switch (protocol) {
    case "chat":
      return "/v1/chat/completions";
    case "responses":
      return "/v1/responses";
    case "anthropic":
      return "/v1/messages";
    default:
      return protocol;
  }
};

/**
 * 毫秒的紧凑显示。
 *
 * 一小时内用 ms，再长用 s，超过一分钟用 m ——
 * 同一个表格里混着 800ms 和 73000ms 时，读者要在脑子里做除法才能比大小。
 */
const formatDuration = (ms: number) => {
  if (ms < 1000) {
    return `${Math.round(ms)}ms`;
  }
  if (ms < 60_000) {
    return `${(ms / 1000).toFixed(2)}s`;
  }
  const minutes = Math.floor(ms / 60_000);
  return `${minutes}m${Math.round((ms % 60_000) / 1000)}s`;
};

const toIso = (range: DateRange, endOfDay: boolean) => {
  if (!range.from && !range.to) {
    return undefined;
  }
  const base = endOfDay ? range.to || range.from : range.from;
  if (!base) {
    return undefined;
  }
  const [year, month, day] = base.split("-").map(Number);
  const date = new Date(year, month - 1, day, endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0);
  return date.toISOString();
};

const csvEscape = (value: string | number) => {
  const text = String(value);
  return /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
};

/**
 * 把当前这一页导出成 CSV。
 *
 * 为什么导出的是"当前页"而不是"全部筛选结果"：
 * 全量导出要再打一次接口拉几千行，而用户真正需要的往往是"把这一屏拿去对账"。
 * 所以按钮上直接写「导出本页」，不让人误以为是全量 ——
 * 界面文案必须和实际行为一致，否则用户会拿着残缺的表格去核账。
 */
const exportRows = (logs: UsageLog[]) => {
  const header = [
    "时间",
    "模型",
    "渠道",
    "Key",
    "输入",
    "缓存命中",
    "输出",
    "合计",
    "成本",
    "首字(ms)",
    "耗时(ms)",
    "速度(token/s)",
    "重试",
    "状态",
  ];
  const rows = logs.map((log) => [
    new Date(log.ts).toISOString(),
    log.model,
    log.channelId,
    log.keyName ?? "",
    log.promptTokens,
    log.cachedTokens,
    log.completionTokens,
    log.totalTokens,
    log.billedCost.toFixed(6),
    log.ttftMs ?? "",
    log.totalMs,
    // 近似值在 CSV 里也用 `≈` 前缀，导出的表也能一眼看出哪条不能直接比。
    log.tokensPerSecond === null
      ? ""
      : `${log.speedEstimated ? "≈" : ""}${log.tokensPerSecond.toFixed(1)}`,
    log.attempts,
    log.status,
  ]);

  const csv = [header, ...rows]
    .map((row) => row.map(csvEscape).join(","))
    .join("\n");

  // BOM 让 Excel 正确识别 UTF-8（否则中文列名是乱码）。
  const blob = new Blob([`\uFEFF${csv}`], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `relayai-usage-${Date.now()}.csv`;
  link.click();
  URL.revokeObjectURL(url);
};

export const UsagePage = () => {
  const [logs, setLogs] = useState<UsageLog[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [range, setRange] = useState<DateRange>({ from: "", to: "" });
  const [channelFilter, setChannelFilter] = useState(ALL);
  const [statusFilter, setStatusFilter] = useState(ALL);
  /** 是否有任何筛选条件 —— 空态文案要据此区分"没数据"和"筛掉了"。 */
  const hasFilters =
    Boolean(range.from) || Boolean(range.to) || channelFilter !== ALL || statusFilter !== ALL;
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    void listChannels()
      .then((response) => setChannels(response.data ?? []))
      .catch(() => setChannels([]));
  }, []);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listUsage({
        from: toIso(range, false),
        to: toIso(range, true),
        channelId: channelFilter === ALL ? undefined : channelFilter,
        status: statusFilter === ALL ? undefined : (statusFilter as UsageStatus),
        page,
        pageSize: PAGE_SIZE,
      });
      setLogs(response.data?.list ?? []);
      setTotal(response.data?.total ?? 0);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, [channelFilter, page, range, statusFilter]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const columns: Array<Column<UsageLog>> = [
    {
      key: "key",
      header: "API 密钥",
      width: "96px",
      render: (log) =>
        log.keyName ? (
          <span
            className="inline-flex max-w-full items-center rounded bg-accent px-1.5 py-0.5 text-sm text-accent-foreground"
            title={log.keyName}
          >
            <span className="truncate">{log.keyName}</span>
          </span>
        ) : (
          <span className="text-sm text-muted-foreground">—</span>
        ),
    },
    {
      key: "model",
      header: "模型",
      // 模型名是这一页最有信息量的一列，给它最大的一份。
      // 必须用**具体 px**：`table-fixed` 下 % 与 px 混用时像素列先占满，
      // 百分比列会被压到 0（实测模型名整列消失）。
      width: "196px",
      render: (log) =>
        log.isRejection ? (
          <span className="text-sm text-muted-foreground">—</span>
        ) : (
          <span className="block truncate font-mono text-sm font-medium text-primary" title={log.model}>
            {log.model}
          </span>
        ),
    },
    {
      key: "endpoint",
      header: "端点",
      width: "150px",
      hideOnMobile: true,
      render: (log) =>
        // 未调度的行没有真正打到上游，端点信息对排查没有价值，
        // 显示成「—」比显示一个"看起来成功的入站地址"更诚实。
        log.isRejection ? (
          <span className="text-sm text-muted-foreground">—</span>
        ) : (
          <span
            className="block truncate font-mono text-sm text-muted-foreground"
            title={protocolLabel(log.protocol)}
          >
            {protocolLabel(log.protocol)}
          </span>
        ),
    },
    {
      key: "channel",
      header: "分组",
      width: "96px",
      render: (log) => (
        <span className="inline-flex max-w-full items-center rounded bg-muted px-1.5 py-0.5 font-mono text-sm text-muted-foreground">
          <span className="truncate">{log.channelId || "—"}</span>
        </span>
      ),
    },
    {
      key: "type",
      header: "类型",
      width: "76px",
      align: "center",
      hideOnMobile: true,
      render: (log) => (
        <span
          className={cn(
            "inline-flex items-center rounded px-1.5 py-0.5 text-sm",
            log.stream
              ? "bg-success/10 text-success"
              : "bg-muted text-muted-foreground",
          )}
        >
          {log.stream ? "流式" : "非流式"}
        </span>
      ),
    },
    /*
      Token 三档**叠在一格**里（参考 sub2api 的信息密度）。

      与早先"拆成四列"的取舍：
        · 拆开后每列都要占宽度，12 列把表格挤到必须横向滚动；
        · 三档本来是一组数（缓存是输入的一部分、合计 = 输入 + 输出），
          竖排在一起反而更容易横向对比，也省出空间给端点/类型。
      用 ↓ ↑ ⛁ 三个符号区分输入/输出/缓存，比纯文字更省空间。
    */
    {
      key: "tokens",
      header: "TOKEN",
      width: "120px",
      bodyAlign: "left",
      hideOnMobile: true,
      render: (log) => {
        if (log.isRejection) {
          return <span className="text-sm text-muted-foreground">—</span>;
        }
        return (
          <div className="flex flex-col gap-0.5 text-sm leading-4 tabular-nums">
            <span className="flex items-center gap-2">
              {/* 输入偏冷、输出偏暖：这两项相加就是总量，靠颜色区分比靠箭头更省神。 */}
              <span className="text-chart-1">
                ↓ {compactNumber(log.promptTokens)}
              </span>
              <span className="text-chart-2">
                ↑ {compactNumber(log.completionTokens)}
              </span>
            </span>
            <span
              className={cn(
                "flex items-center gap-1 font-medium",
                log.cachedTokens > 0 ? "text-success" : "text-foreground",
              )}
              title={
                log.cachedTokens > 0
                  ? `缓存命中 ${compactNumber(log.cachedTokens)}，占输入 ${(
                      (log.cachedTokens / Math.max(log.promptTokens, 1)) *
                      100
                    ).toFixed(0)}%`
                  : "没有命中缓存"
              }
            >
              ⛁ {compactNumber(log.totalTokens)}
              {log.usageEstimated ? (
                <span className="text-[10px] text-warning" title="上游没给 usage，这里是估算值">
                  ≈
                </span>
              ) : null}
            </span>
          </div>
        );
      },
    },
    {
      key: "cost",
      header: "费用",
      width: "96px",
      align: "center",
      hideOnMobile: true,
      render: (log) =>
        log.isRejection ? (
          <span className="text-sm text-muted-foreground">—</span>
        ) : (
          <CostBreakdown log={log} />
        ),
    },
    /*
      延迟三档也叠成一格：首字 / 总耗时 / 输出速度。
      它们回答同一个问题（"这次快不快"），分开占三列会把表格撑散。
    */
    {
      key: "latency",
      header: "延迟",
      width: "132px",
      bodyAlign: "left",
      hideOnMobile: true,
      render: (log) => {
        if (log.isRejection) {
          return <span className="text-sm text-muted-foreground">—</span>;
        }
        return (
          <div className="flex flex-col gap-0.5 text-sm leading-4 tabular-nums">
            <span className="flex items-baseline justify-between gap-2">
              <span className="text-muted-foreground">首字</span>
              {/*
                首字延迟三档：≤5s 绿 / 5–15s 橙 / >15s 红。

                为什么不用 1 秒：长上下文与推理模型的首字本来就慢，
                1 秒就标色会让整列都是橙的，等于没有信号。
                5 秒是"明显变慢"，15 秒是"用户已经在怀疑卡死了"。
              */}
              <span
                className={cn(
                  "font-medium",
                  log.ttftMs === null
                    ? ""
                    : log.ttftMs > 15_000
                      ? "text-destructive"
                      : log.ttftMs > 5_000
                        ? "text-warning"
                        : "text-success",
                )}
              >
                {log.ttftMs === null ? "—" : formatDuration(log.ttftMs)}
              </span>
            </span>
            <span className="flex items-baseline justify-between gap-2">
              <span className="text-muted-foreground">总耗时</span>
              <span>{formatDuration(log.totalMs)}</span>
            </span>
            <span className="flex items-baseline justify-between gap-2">
              <span className="text-muted-foreground">输出</span>
              <span
                className={cn(
                  "tabular-nums",
                  log.speedEstimated ? "text-muted-foreground" : "text-chart-4",
                )}
                title={
                  log.speedEstimated
                    ? "近似值：这条没有首字计时，用总时长估算，等首字的时间也算在内，因此偏低"
                    : "从首字到结束的输出速度"
                }
              >
                {log.tokensPerSecond === null
                  ? "—"
                  : `${log.speedEstimated ? "≈" : ""}${log.tokensPerSecond.toFixed(1)} token/s`}
              </span>
            </span>
          </div>
        );
      },
    },
    {
      key: "ts",
      header: "时间",
      width: "112px",
      align: "center",
      render: (log) => (
        <span className="text-sm text-muted-foreground tabular-nums">
          {new Date(log.ts).toLocaleString("zh-CN", {
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
          })}
        </span>
      ),
    },
    {
      key: "status",
      header: "状态",
      width: "88px",
      render: (log) => (
        <div className="flex flex-col items-start gap-0.5">
          <StatusBadge tone={statusTone(log.status)} label={statusLabel(log.status)} />
          {/* 重试次数只在真的重试过时显示：每行都写"1"是噪音。 */}
          {log.attempts > 1 ? (
            <span className="text-[10px] text-warning">重试 {log.attempts - 1} 次</span>
          ) : null}
        </div>
      ),
    },
  ];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-44">
          <DatePicker
            value={range}
            onChange={(next) => {
              setRange(next);
              setPage(1);
            }}
          />
        </div>

        <div className="w-36">
          <Select
            value={channelFilter}
            onValueChange={(value) => {
              setChannelFilter(value);
              setPage(1);
            }}
          >
            <SelectTrigger aria-label="按渠道筛选">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>全部渠道</SelectItem>
              {channels.map((channel) => (
                <SelectItem key={channel.id} value={channel.id}>
                  {channel.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="w-32">
          <Select
            value={statusFilter}
            onValueChange={(value) => {
              setStatusFilter(value);
              setPage(1);
            }}
          >
            <SelectTrigger aria-label="按状态筛选">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>全部状态</SelectItem>
              <SelectItem value="completed">成功</SelectItem>
              <SelectItem value="failed">失败</SelectItem>
              <SelectItem value="rejected">未调度</SelectItem>
              <SelectItem value="client_aborted">客户端中断</SelectItem>
              <SelectItem value="upstream_aborted">上游断流</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setReloadKey((value) => value + 1)}
            disabled={isLoading}
          >
            <RefreshCw
              className={cn(isLoading && "animate-spin motion-reduce:animate-none")}
              aria-hidden="true"
            />
            刷新
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => exportRows(logs)}
            disabled={logs.length === 0}
            title="导出当前页的 50 条记录"
          >
            <Download aria-hidden="true" />
            导出本页
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox
        isLoading={isLoading && logs.length === 0}
        text="正在加载请求记录..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={logs}
          fixed
          striped
          rowKey={(log) => String(log.id)}
          empty={
            /*
              空态文案要说准"为什么没有记录"，而不是默认叫人去建密钥。

              之前无条件显示「先创建一个密钥」：已经有一堆 Key 的人看到它
              只会觉得「我他妈不是有吗」。空态该解释原因，不该乱给动作。
            */
            <EmptyState
              icon={FileClock}
              title="还没有请求记录"
              description={
                hasFilters
                  ? "当前筛选条件下没有记录。换个时间范围或状态再试。"
                  : "客户端通过网关发出请求后，这里会出现逐条明细。"
              }
            />
          }
        />
      </MyBox>

      <Pagination
        page={page}
        pageSize={PAGE_SIZE}
        total={total}
        onPageChange={setPage}
        onPageSizeChange={() => {}}
        disabled={isLoading}
      />
    </div>
  );
};
