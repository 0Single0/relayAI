import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Activity,
  Coins,
  Database,
  Gauge,
  RefreshCw,
  Wallet,
  Zap,
} from "lucide-react";
import { getDashboardOverview } from "@/api/usage";
import { getGatewayStatus } from "@/api/system";
import type {
  DashboardOverview,
  DashboardTotals,
  GatewayStatus,
} from "@/api/types";
import { ChartPlaceholder, TrendChart } from "@/components/common/Charts";
import {
  DonutWithLegend,
  type DonutDatum,
} from "@/components/common/Charts/DonutWithLegend";
import { compactNumber, fillSeriesGaps } from "@/lib/charts";
import { DatePicker, type DateRange } from "@/components/common/DatePicker";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { StatCard } from "@/components/common/StatCard";
import { Button } from "@/components/ui/button";
import { useAuthStore } from "@/store/auth";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const emptyTotals: DashboardTotals = {
  requests: 0,
  successRate: 0,
  promptTokens: 0,
  completionTokens: 0,
  totalTokens: 0,
  cachedTokens: 0,
  upstreamCost: 0,
  billedCost: 0,
  avgTtftMs: null,
  avgLatencyMs: null,
};

/** 趋势图的时间范围。24 小时用小时桶，更长的用天桶。 */
const RANGES = [
  { hours: 24, label: "24 小时", bucket: "hour" },
  { hours: 24 * 7, label: "7 天", bucket: "day" },
  { hours: 24 * 30, label: "30 天", bucket: "day" },
] as const;

const formatCost = (value: number) =>
  value >= 1 ? `$${value.toFixed(2)}` : `$${value.toFixed(4)}`;

const formatMs = (value: number | null) =>
  value === null ? "—" : value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${value}ms`;

const formatCount = (value: number) => value.toLocaleString("zh-CN");

export const DashboardPage = () => {
  const user = useAuthStore((state) => state.user);
  const [overview, setOverview] = useState<DashboardOverview | null>(null);
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [rangeIndex, setRangeIndex] = useState(0);
  const [customRange, setCustomRange] = useState<DateRange>({ from: "", to: "" });
  const [reloadKey, setReloadKey] = useState(0);
  // 查询区间要与快照一起记住：补零需要和后端**完全相同的** from/to，
  // 否则前后端各用一次 `new Date()`，边界桶会错位一格。
  const [window, setWindow] = useState<{ from: Date; to: Date } | null>(null);

  const range = RANGES[rangeIndex];

  // 自定义范围是否生效。只有 from 与 to 都填了才算 —— 只填起点时
  // 仍然按快捷档位查询，避免用户点第一下就触发一次跨度巨大的查询。
  const hasCustomRange = customRange.from !== "" && customRange.to !== "";

  // 自定义范围按自然日：from 从当天 00:00 起，to 到当天 23:59:59 止。
  // 直接用日期字符串当边界会漏掉最后一天的大部分时间。
  const customBounds = useMemo(() => {
    if (!hasCustomRange) {
      return null;
    }
    const start = new Date(`${customRange.from}T00:00:00`);
    const end = new Date(`${customRange.to}T23:59:59`);
    if (Number.isNaN(start.getTime()) || Number.isNaN(end.getTime())) {
      return null;
    }
    return { start, end };
  }, [customRange.from, customRange.to, hasCustomRange]);

  // 自定义跨度超过 3 天就按天聚合，否则横轴会挤成一片。
  const bucket = useMemo<"hour" | "day">(() => {
    if (!customBounds) {
      return range.bucket;
    }
    return customBounds.end.getTime() - customBounds.start.getTime() > 3 * 864e5
      ? "day"
      : "hour";
  }, [customBounds, range.bucket]);

  const load = useCallback(async () => {
    setIsLoading(true);
    const now = customBounds?.end ?? new Date();
    const from =
      customBounds?.start ?? new Date(now.getTime() - range.hours * 3600 * 1000);

    try {
      const [overviewResponse, statusResponse] = await Promise.all([
        getDashboardOverview({
          from: from.toISOString(),
          to: now.toISOString(),
          bucket,
        }),
        getGatewayStatus().catch(() => null),
      ]);
      setOverview(overviewResponse.data ?? null);
      setStatus(statusResponse?.data ?? null);
      setWindow({ from, to: now });
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
      setOverview(null);
    } finally {
      setIsLoading(false);
    }
  }, [bucket, customBounds, range.hours]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const today = overview?.today ?? emptyTotals;
  const total = overview?.total ?? emptyTotals;

  // 补零成连续时间轴：后端只返回有数据的桶，直接画会把 4 个点等距摊满，
  // 看起来"中间的时间段消失了"。补 0 之后空白时段是真实信息。
  const trendRows = useMemo(
    () =>
      window ? fillSeriesGaps(overview?.series ?? [], window.from, window.to, bucket) : [],
    [bucket, overview?.series, window],
  );

  // 今日分布：横轴固定为 00:00–当前，没有请求的小时补 0。
  const hourRows = useMemo(() => {
    if (!window) {
      return [];
    }
    const startOfToday = new Date(window.to);
    startOfToday.setHours(0, 0, 0, 0);
    return fillSeriesGaps(
      overview?.todayByHour ?? [],
      startOfToday,
      window.to,
      "hour",
    );
  }, [overview?.todayByHour, window]);

  // 卡片右侧迷你曲线：用补零后的连续序列，取最后 24 个桶。
  const sparks = useMemo(() => {
    const tail = <T,>(rows: T[]) => rows.slice(-24);
    const requests = tail(hourRows).map((row) => row.requests);
    const tokens = tail(hourRows).map((row) => row.totalTokens);
    const ttft = tail(hourRows).map((row) => row.avgTtftMs ?? 0);
    const cacheRate = tail(hourRows).map((row) =>
      row.requests > 0 ? row.successRate * 100 : 0,
    );
    return {
      requests,
      tokens,
      ttft,
      cacheRate,
      // 累计类指标没有"随时间变化"的口径，用近期趋势表达走势。
      totalRequests: requests,
      totalTokens: tokens,
    };
  }, [hourRows]);

  // 渠道构成：按请求数排序，颜色只从主题 token 取。
  const channelDonut = useMemo<DonutDatum[]>(() => {
    const palette = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"];
    return (overview?.byChannel ?? []).slice(0, 5).map((bucket, index) => ({
      name: bucket.label,
      value: bucket.requests,
      color: palette[index % palette.length],
    }));
  }, [overview?.byChannel]);

  const protocolDonut = useMemo<DonutDatum[]>(() => {
    const labels: Record<string, string> = {
      chat: "Chat Completions",
      anthropic: "Anthropic Messages",
      responses: "OpenAI Responses",
    };
    const palette = ["var(--chart-1)", "var(--chart-2)", "var(--chart-4)"];
    return (overview?.byProtocol ?? []).map((bucket, index) => ({
      name: labels[bucket.protocol] ?? bucket.protocol,
      value: bucket.requests,
      color: palette[index % palette.length],
    }));
  }, [overview?.byProtocol]);

  const topModels = useMemo(
    () => (overview?.byModel ?? []).slice(0, 6),
    [overview?.byModel],
  );

  /**
   * 成功率图的取数：把 `successRate`（0–1）换成百分比。
   *
   * 空桶（没有请求）要**剔除**而不是记成 0%：
   * 没有请求的时段谈不上"成功率 0%"，画进去会在图上压出一条到底的假谷。
   */
  const successRows = useMemo(
    () =>
      trendRows
        .filter((row) => row.requests > 0)
        .map((row) => ({ ...row, successPercent: row.successRate * 100 })),
    [trendRows],
  );

  /**
   * 延迟图的取数：只保留**真的测到延迟**的桶。
   *
   * 为什么必须过滤而不能留着 `null`：空桶的延迟是 `null`（"没请求"≠"0ms"），
   * 而 recharts 的 `monotone` 曲线遇到 `null` 会断开。当有值的桶彼此不相邻时
   * （本例就是这样：只有 7 个桶有请求，其余 17 个是空），
   * 每个点都是孤立的 —— 一个线段都画不出来，整张图看起来是空的。
   *
   * 剔掉空桶后横轴会跳过空白时段。这是可接受的取舍：延迟图要回答的是
   * "有请求的时候有多慢"，不是"什么时候没有请求"（那是请求量图的职责）。
   */
  const latencyRows = useMemo(
    () =>
      trendRows.filter(
        (row) => row.avgLatencyMs !== null || row.avgTtftMs !== null,
      ),
    [trendRows],
  );

  const gatewayHealthy = status?.running === true;

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {/* 工具条 */}
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge
          tone={gatewayHealthy ? "success" : status ? "danger" : "neutral"}
          label={
            gatewayHealthy
              ? `网关运行中 · ${status?.addr ?? "-"}`
              : status
                ? "网关未运行"
                : "网关状态未知"
          }
        />
        <span className="text-xs text-muted-foreground">
          并发 {status ? status.concurrency.global - status.concurrency.available : 0}/
          {status?.concurrency.global ?? 0}
        </span>

        <div className="ml-auto flex flex-wrap items-center gap-2">
          {/*
            自定义日期范围。选择后以它为准（覆盖上面的快捷档位）；
            点快捷档位则清空自定义范围，回到滚动窗口。
            两者同时存在是刻意的：快捷档位覆盖 90% 的日常查看，
            自定义范围用于"查一下某个具体那天出了什么问题"。
          */}
          <div className="w-[15.5rem]">
            <DatePicker
              value={customRange}
              onChange={(next) => {
                setCustomRange(next);
                // 只选了起点还没选终点时不要立刻生效：
                // 那会把区间变成"某一天到现在"，用户会觉得页面在乱跳。
                if (next.from && next.to) {
                  setReloadKey((value) => value + 1);
                }
              }}
              label="自定义"
            />
          </div>

          <div
            className="inline-flex items-center rounded-lg border bg-card p-0.5"
            role="group"
            aria-label="趋势时间范围"
          >
            {RANGES.map((item, index) => (
              <button
                key={item.label}
                type="button"
                onClick={() => {
                  setRangeIndex(index);
                  // 快捷档位与自定义范围互斥，否则用户分不清哪个在生效。
                  setCustomRange({ from: "", to: "" });
                }}
                aria-pressed={rangeIndex === index && !hasCustomRange}
                className={cn(
                  "rounded-md px-2.5 py-1 text-xs transition-colors",
                  "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                  rangeIndex === index && !hasCustomRange
                    ? "bg-accent font-medium text-accent-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {item.label}
              </button>
            ))}
          </div>

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
        </div>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
          hint="确认后端与网关都已启动，且数据库可连接。"
        />
      ) : null}

      <MyBox isLoading={isLoading && overview === null} text="正在加载统计数据...">
        {/*
          指标卡的顺序按"用户最关心什么"排，不是按数据是否好算：
          余额 → 今日消费 → 今日 Token → 累计 Token → 今日请求 → 首字延迟。

          余额排第一：这是唯一一个"再不动手就要停机"的数字。
          原来的「缓存命中 / 累计成功率 / 累计请求」三张卡去掉了 ——
          它们是好看但不可行动（看到了也不能做什么），
          而首字延迟是"用户抱怨慢"时第一个要看的数，所以留着。
        */}
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
          <StatCard
            label="账户余额"
            value={`¥${(user?.balance ?? 0).toFixed(2)}`}
            hint={user ? `累计充值 ¥${user.totalRecharged.toFixed(2)}` : "未登录"}
            icon={Wallet}
            tone="green"
          />
          <StatCard
            label="今日消费"
            value={formatCost(today.billedCost)}
            hint={`上游实付 ${formatCost(today.upstreamCost)}`}
            icon={Coins}
            tone="amber"
            spark={sparks.tokens}
            trendLabel="24 小时"
          />
          <StatCard
            label="今日 Token"
            value={compactNumber(today.totalTokens)}
            hint={`输入 ${compactNumber(today.promptTokens)} · 输出 ${compactNumber(today.completionTokens)}`}
            icon={Zap}
            tone="violet"
            spark={sparks.tokens}
            trendLabel="24 小时"
          />
          <StatCard
            label="累计 Token"
            value={compactNumber(total.totalTokens)}
            hint={`输入 ${compactNumber(total.promptTokens)} · 输出 ${compactNumber(total.completionTokens)}`}
            icon={Database}
            tone="violet"
            spark={sparks.totalTokens}
          />
          <StatCard
            label="今日请求"
            value={formatCount(today.requests)}
            hint={`成功率 ${(today.successRate * 100).toFixed(1)}%`}
            icon={Activity}
            tone="primary"
            spark={sparks.requests}
            trendLabel="24 小时"
          />
          <StatCard
            label="今日首字延迟"
            value={formatMs(today.avgTtftMs)}
            hint={`平均耗时 ${formatMs(today.avgLatencyMs)}`}
            icon={Gauge}
            tone="primary"
            spark={sparks.ttft}
            trendLabel="24 小时"
          />
        </div>

        {/*
          两张趋势图并排：左边只讲"请求量"，右边讲"Token 构成"。
          拆开而不是画一张双 Y 轴图：请求数（个位到千）与 Token（百万级）
          量纲差三四个数量级，塞进一个坐标系里必有一条被压成贴着轴的直线。
        */}
        <div className="mt-3 grid grid-cols-1 gap-3 xl:grid-cols-2">
          <PageSection
            title="请求量"
            description="每小时请求数，空时段补 0 以保留真实的低谷"
            bodyClassName="pt-2"
          >
            {trendRows.length === 0 ? (
              <ChartPlaceholder text="该区间还没有请求" />
            ) : (
              <TrendChart
                data={trendRows}
                xKey="label"
                height={260}
                series={[
                  {
                    key: "requests",
                    label: "请求数",
                    color: "var(--chart-1)",
                    unit: "次",
                    formatValue: (value) => `${value.toLocaleString("zh-CN")} 次`,
                  },
                ]}
                tooltipFooter={(row) => {
                  const failed = Number(row.failed ?? 0);
                  const requests = Number(row.requests ?? 0);
                  return failed > 0
                    ? `其中失败 ${failed.toLocaleString("zh-CN")} 次 · 成功率 ${
                        requests > 0 ? ((1 - failed / requests) * 100).toFixed(1) : "—"
                      }%`
                    : `全部成功 · 成功率 100%`;
                }}
              />
            )}
          </PageSection>

          <div className="min-w-0">
            <PageSection
              title="使用趋势"
              description="四条曲线共用时间轴；输入 + 输出即总量，缓存命中含在输入里"
              bodyClassName="pt-2"
            >
              {trendRows.length === 0 ? (
                <ChartPlaceholder text="该区间还没有请求" />
              ) : (
                <TrendChart
                  data={trendRows}
                  xKey="label"
                  height={260}
                  series={[
                    {
                      key: "promptTokens",
                      label: "输入",
                      color: "var(--chart-1)",
                      unit: "token",
                      formatValue: (value) => `${value.toLocaleString("zh-CN")} token`,
                    },
                    {
                      key: "cachedTokens",
                      label: "缓存命中",
                      color: "var(--chart-2)",
                      unit: "token",
                      formatValue: (value) => `${value.toLocaleString("zh-CN")} token`,
                    },
                    {
                      key: "completionTokens",
                      label: "输出",
                      color: "var(--chart-3)",
                      unit: "token",
                      formatValue: (value) => `${value.toLocaleString("zh-CN")} token`,
                    },
                    {
                      key: "totalTokens",
                      label: "总量",
                      color: "var(--chart-4)",
                      unit: "token",
                      formatValue: (value) => `${value.toLocaleString("zh-CN")} token`,
                    },
                  ]}
                  tooltipFooter={(row) =>
                    `该时段请求 ${formatCount(Number(row.requests ?? 0))} 次`
                  }
                />
              )}
            </PageSection>
          </div>
        </div>

        {/* 端点分布 / 渠道构成 / 模型排行：一行三块 */}
        <div className="mt-3 grid grid-cols-1 gap-3 xl:grid-cols-3">
          <PageSection title="端点分布" description="三个入口各自承担多少流量">
            <DonutWithLegend
              data={protocolDonut}
              centerLabel="请求"
              centerValue={compactNumber(today.requests)}
              size={120}
              trailingHeader="占比"
              emptyText="今天还没有请求"
            />
          </PageSection>

          <PageSection title="渠道构成" description="今日请求按渠道拆分">
            <DonutWithLegend
              data={channelDonut}
              centerLabel="请求"
              centerValue={compactNumber(today.requests)}
              size={120}
              trailingHeader="占比"
              emptyText="今天还没有请求"
            />
          </PageSection>

          <PageSection
            title="模型排行"
            description="今日按请求数排序"
            bodyClassName="p-0"
          >
            {topModels.length === 0 ? (
              <EmptyState
                icon={Activity}
                title="还没有模型调用"
                description="接入客户端并发出第一个请求后，这里会出现排行。"
              />
            ) : (
              <ul className="divide-y">
                {topModels.map((model, index) => {
                  const share = today.requests > 0 ? model.requests / today.requests : 0;
                  return (
                    <li
                      key={model.key}
                      className="flex items-center gap-2 px-3 py-2 hover:bg-muted/30"
                    >
                      <span
                        className={cn(
                          "inline-flex size-5 shrink-0 items-center justify-center rounded-md text-[11px] font-semibold tabular-nums",
                          index === 0
                            ? "bg-primary text-primary-foreground"
                            : index === 1
                              ? "bg-primary/15 text-primary"
                              : index === 2
                                ? "bg-primary/10 text-primary"
                                : "bg-muted text-muted-foreground",
                        )}
                      >
                        {index + 1}
                      </span>
                      <span className="min-w-0 flex-1 truncate font-mono text-xs">
                        {model.label}
                      </span>
                      <span className="w-10 shrink-0 text-right text-xs tabular-nums">
                        {formatCount(model.requests)}
                      </span>
                      <span className="w-10 shrink-0 text-right text-[11px] tabular-nums text-muted-foreground">
                        {(share * 100).toFixed(0)}%
                      </span>
                      <span
                        className={cn(
                          "w-12 shrink-0 text-right text-[11px] tabular-nums",
                          model.successRate >= 0.99
                            ? "text-success"
                            : model.successRate >= 0.9
                              ? "text-warning"
                              : "text-destructive",
                        )}
                      >
                        {(model.successRate * 100).toFixed(0)}%
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
          </PageSection>
        </div>

        {/*
          第二组图：延迟与成功率。

          为什么这两条值得单独画：前两张图讲"用了多少"，
          这两张讲"好不好用"。用户抱怨"最近变慢了/老是失败"时，
          答案只能从这里看出来 —— 用量图完全看不出来。
        */}
        <div className="mt-3 grid grid-cols-1 gap-3 xl:grid-cols-2">
          <PageSection
            title="响应耗时"
            description="首字延迟与整体耗时；空时段不画点（没有请求就没有延迟）"
            bodyClassName="pt-2"
          >
            {latencyRows.length === 0 ? (
              <ChartPlaceholder text="该区间还没有请求" />
            ) : (
              <TrendChart
                data={latencyRows}
                xKey="label"
                height={220}
                compactAxis={false}
                series={[
                  {
                    key: "avgTtftMs",
                    label: "首字延迟",
                    color: "var(--chart-3)",
                    formatValue: (value) => formatMs(value),
                  },
                  {
                    key: "avgLatencyMs",
                    label: "总耗时",
                    color: "var(--chart-2)",
                    formatValue: (value) => formatMs(value),
                  },
                ]}
                tooltipFooter={(row) =>
                  `该时段 ${formatCount(Number(row.requests ?? 0))} 次请求`
                }
              />
            )}
          </PageSection>

          <PageSection
            title="成功率"
            description="按时间桶统计；失败包含上游错误与客户端中断"
            bodyClassName="pt-2"
          >
            {trendRows.length === 0 ? (
              <ChartPlaceholder text="该区间还没有请求" />
            ) : (
              <TrendChart
                data={successRows}
                xKey="label"
                height={220}
                compactAxis={false}
                series={[
                  {
                    key: "successPercent",
                    label: "成功率",
                    color: "var(--chart-4)",
                    formatValue: (value) => `${value.toFixed(1)}%`,
                  },
                ]}
                tooltipFooter={(row) =>
                  `失败 ${formatCount(Number(row.failed ?? 0))} / 共 ${formatCount(
                    Number(row.requests ?? 0),
                  )} 次`
                }
              />
            )}
          </PageSection>
        </div>

        {/*
          第三组：错误分布 + 延迟分位。

          这两块是排障入口，补的是前面图表答不了的问题：
            · 成功率只说"有问题"，错误分布才说"问题在哪"；
            · 平均延迟会被长尾拉偏（99 个快请求 + 1 个超时，平均看起来正常），
              分位才反映用户实际感受到的"卡"。
        */}
        <div className="mt-3 grid grid-cols-1 gap-3 xl:grid-cols-2">
          <PageSection
            title="错误分布"
            description="按错误类型分组；占比最高的一类通常就是根因"
            bodyClassName="pt-2"
          >
            {(overview?.errors ?? []).length === 0 ? (
              <ChartPlaceholder text="该区间没有失败请求" />
            ) : (
              <ul className="flex flex-col divide-y">
                {(overview?.errors ?? []).map((item) => {
                  const total = (overview?.errors ?? []).reduce((sum, row) => sum + row.count, 0);
                  const percent = total > 0 ? (item.count / total) * 100 : 0;
                  return (
                    <li key={item.kind} className="flex flex-col gap-1 py-2">
                      <div className="flex items-baseline justify-between gap-2">
                        <span className="truncate font-mono text-xs">{item.kind}</span>
                        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                          {formatCount(item.count)} 次 · {percent.toFixed(0)}%
                        </span>
                      </div>
                      {/* 占比条：一眼看出哪类占大头，比读百分比快 */}
                      <div className="h-1 overflow-hidden rounded-full bg-muted">
                        <div
                          className="h-full rounded-full bg-destructive/70"
                          style={{ width: `${Math.max(percent, 2)}%` }}
                        />
                      </div>
                      {item.sample ? (
                        <p
                          className="truncate text-[11px] text-muted-foreground"
                          title={item.sample}
                        >
                          {item.sample}
                        </p>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            )}
          </PageSection>

          <PageSection
            title="延迟分位"
            description={`窗口内 ${formatCount(overview?.percentiles.sample ?? 0)} 次成功请求；平均值会被长尾拉偏，分位更接近体感`}
            bodyClassName="pt-2"
          >
            {(overview?.percentiles.sample ?? 0) === 0 ? (
              <ChartPlaceholder text="该区间还没有成功请求" />
            ) : (
              <div className="flex flex-col gap-4 py-1">
                <PercentileRow
                  label="首字延迟"
                  p50={overview?.percentiles.ttftP50 ?? null}
                  p95={overview?.percentiles.ttftP95 ?? null}
                  p99={overview?.percentiles.ttftP99 ?? null}
                />
                <PercentileRow
                  label="总耗时"
                  p50={overview?.percentiles.totalP50 ?? null}
                  p95={overview?.percentiles.totalP95 ?? null}
                  p99={overview?.percentiles.totalP99 ?? null}
                />
              </div>
            )}
          </PageSection>
        </div>
      </MyBox>
    </div>
  );
};

/** 一行分位：P50 / P95 / P99。 */
const PercentileRow = ({
  label,
  p50,
  p95,
  p99,
}: {
  label: string;
  p50: number | null;
  p95: number | null;
  p99: number | null;
}) => {
  const cells = [
    { key: "p50", label: "P50", value: p50, hint: "一半请求比它快" },
    { key: "p95", label: "P95", value: p95, hint: "95% 请求比它快" },
    { key: "p99", label: "P99", value: p99, hint: "最慢的 1%；持续偏高说明有长尾" },
  ];

  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <div className="grid grid-cols-3 gap-2">
        {cells.map((cell) => (
          <div
            key={cell.key}
            title={cell.hint}
            className="flex flex-col gap-0.5 rounded-lg bg-muted/50 px-2.5 py-1.5"
          >
            <span className="text-[11px] text-muted-foreground">{cell.label}</span>
            <span className="text-sm font-medium tabular-nums">
              {cell.value === null ? "—" : formatMs(cell.value)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
};
