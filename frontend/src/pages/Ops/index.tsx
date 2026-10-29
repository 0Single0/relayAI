import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, RefreshCw, ServerCog, Timer } from "lucide-react";
import { APIRequest } from "@/utils/request";
import { ChartPlaceholder, TrendChart } from "@/components/common/Charts";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";

type OpsPercentiles = {
  p50: number | null;
  p95: number | null;
  p99: number | null;
  avg: number | null;
  sample: number;
};

type OpsTrendPoint = {
  label: string;
  requests: number;
  completed: number;
  failed: number;
  retried: number;
  ttftP50: number | null;
  ttftP95: number | null;
  ttftP99: number | null;
  totalP50: number | null;
  totalP95: number | null;
  otpsP50: number | null;
  otpsP95: number | null;
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
};

type OpsMetrics = {
  windowMinutes: number;
  from: string;
  to: string;
  ttft: OpsPercentiles;
  total: OpsPercentiles;
  otps: OpsPercentiles;
  requests: number;
  completed: number;
  failed: number;
  rejections: number;
  retried: number;
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  speedSample: number;
  trend: OpsTrendPoint[];
};

type OpsOverview = {
  gatewayOnline: boolean;
  gatewayError?: string;
  checkedAt: string;
  gateway?: {
    running: boolean;
    addr: string;
    version: string;
    uptimeMs: number;
    auth: string;
  };
  concurrency?: {
    global: number;
    available: number;
    used: number;
    accountInflight: number;
    accountCapacity: number;
  };
  counters?: { requests: number; failures: number; retries: number };
  matrix?: {
    routes: number;
    live: number;
    builtin: number;
    ageMs: number;
    ttlSeconds: number;
  };
  accounts?: {
    total: number;
    ready: number;
    cooling: number;
    noCredential: number;
    disabled: number;
  };
  metrics?: OpsMetrics | null;
  warnings?: string[];
};

const WINDOWS = [
  { minutes: 60, label: "1 小时", bucket: 5 },
  { minutes: 360, label: "6 小时", bucket: 15 },
  { minutes: 1440, label: "24 小时", bucket: 30 },
];

const formatCount = (value: number) => value.toLocaleString("zh-CN");

const formatMs = (value: number) =>
  value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${Math.round(value)}ms`;

/**
 * 运维监控。
 *
 * 与仪表盘分工：仪表盘回答"用了多少、好不好用"（面向使用），
 * 这里回答"现在跑得怎么样、哪里在漏"（面向排障）。
 *
 * **一律用图表**：TTFT / OTPS / 吞吐都是随时间波动的量，
 * 一串数字只能说明"此刻是多少"，看不出"是不是刚变差"。
 * 排障要看的是拐点，不是瞬时值。
 *
 * 只有三种情况用纯文字，因为图表达不了：
 *   1. 告警原文（一段话）；
 *   2. 网关离线原因（同上）；
 *   3. 空态提示。
 */
export const OpsPage = () => {
  const [data, setData] = useState<OpsOverview | null>(null);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [windowMinutes, setWindowMinutes] = useState(60);

  const bucket = useMemo(
    () => WINDOWS.find((item) => item.minutes === windowMinutes)?.bucket ?? 5,
    [windowMinutes],
  );

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await APIRequest<OpsOverview>({
        url: "/ops/overview",
        method: "GET",
        params: { windowMinutes, bucketMinutes: bucket },
      });
      setData(response.data ?? null);
      setError("");
    } catch (requestError) {
      setError(requestError instanceof Error ? requestError.message : "加载失败");
    } finally {
      setIsLoading(false);
    }
  }, [bucket, windowMinutes]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  useEffect(() => {
    if (!autoRefresh) {
      return;
    }
    const timer = window.setInterval(() => {
      void load();
    }, 15000);
    return () => window.clearInterval(timer);
  }, [autoRefresh, load]);

  const metrics = data?.metrics ?? null;

  /**
   * 趋势图数据。
   *
   * `null` 必须换成 `undefined`：Recharts 的 `monotone` 遇到 null 会断线，
   * 而"该时段没有样本"和"该时段值为 0"是两件不同的事，不能都当 0。
   */
  const trendRows = useMemo(
    () =>
      (metrics?.trend ?? []).map((point) => ({
        label: point.label,
        requests: point.requests,
        completed: point.completed,
        failed: point.failed,
        retried: point.retried,
        ttftP50: point.ttftP50 ?? undefined,
        ttftP95: point.ttftP95 ?? undefined,
        ttftP99: point.ttftP99 ?? undefined,
        totalP50: point.totalP50 ?? undefined,
        totalP95: point.totalP95 ?? undefined,
        otpsP50: point.otpsP50 ?? undefined,
        otpsP95: point.otpsP95 ?? undefined,
        promptTokens: point.promptTokens,
        completionTokens: point.completionTokens,
        cachedTokens: point.cachedTokens,
      })),
    [metrics],
  );

  const hasTrend = trendRows.length > 0;

  const failureRate = useMemo(() => {
    if (!metrics || metrics.requests <= 0) {
      return null;
    }
    return (metrics.failed / metrics.requests) * 100;
  }, [metrics]);

  const cacheRate = useMemo(() => {
    if (!metrics || metrics.promptTokens <= 0) {
      return null;
    }
    return (metrics.cachedTokens / metrics.promptTokens) * 100;
  }, [metrics]);

  const concurrencyPercent = useMemo(() => {
    const global = data?.concurrency?.global ?? 0;
    if (global <= 0) {
      return 0;
    }
    return Math.min(100, Math.round(((data?.concurrency?.used ?? 0) / global) * 100));
  }, [data]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge
          tone={data?.gatewayOnline ? "success" : data ? "danger" : "neutral"}
          label={data?.gatewayOnline ? "网关运行中" : data ? "网关离线" : "状态未知"}
        />
        {data?.gateway?.addr ? (
          <span className="font-mono text-[11px] text-muted-foreground">
            {data.gateway.addr}
          </span>
        ) : null}

        <div className="ml-auto flex flex-wrap items-center gap-2">
          <div
            className="inline-flex items-center rounded-lg border bg-card p-0.5"
            role="group"
            aria-label="统计窗口"
          >
            {WINDOWS.map((window) => (
              <button
                key={window.minutes}
                type="button"
                onClick={() => setWindowMinutes(window.minutes)}
                aria-pressed={windowMinutes === window.minutes}
                className={cn(
                  "cursor-pointer rounded-md px-2.5 py-1 text-xs transition-colors",
                  "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                  windowMinutes === window.minutes
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:bg-muted",
                )}
              >
                {window.label}
              </button>
            ))}
          </div>

          <Button
            variant={autoRefresh ? "default" : "outline"}
            size="sm"
            onClick={() => setAutoRefresh((value) => !value)}
            aria-pressed={autoRefresh}
          >
            <Timer aria-hidden="true" />
            自动刷新
          </Button>
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
        />
      ) : null}

      <MyBox
        isLoading={isLoading && data === null}
        text="正在读取运行态..."
        className="flex min-h-0 flex-1 flex-col"
      >
        {data === null ? (
          <EmptyState icon={ServerCog} title="运行态不可用" description="后端没能拿到运行数据。" />
        ) : !data.gatewayOnline ? (
          <EmptyState
            icon={AlertTriangle}
            title="网关离线"
            description={
              data.gatewayError ??
              "网关进程没有响应。检查 Rust 网关是否已启动、内部密钥是否一致。"
            }
            action={
              <Button size="sm" onClick={() => setReloadKey((value) => value + 1)}>
                重新检测
              </Button>
            }
          />
        ) : (
          /*
            图表排布：**第一行 2 个、第二行 3 个**。

            用一张 6 栏栅格而不是"两个网格上下堆"：
              · 第一行两个都是"快不快"（延迟 + 出字速度），各占 3 栏 = 等宽；
              · 第二行三个都是"跑了多少、稳不稳"（结果 / 吞吐 / 运行态），各占 2 栏。
            拆成两个独立网格也能做，但两行的左边距会各自对齐，折行时节奏就断了；
            一张网格保证所有卡片共享同一套列宽。

            断点：手机/平板单列 → lg（1024）两列 → xl（1280）起 6 栏（2+3）。
            **两列从 lg 才开始**：768 的平板分两列后每张图只剩 221px，
            X 轴日期标签和卡片标题都开始折行，图变成又高又窄的一条 ——
            折线图没有足够横轴就没有"看拐点"的意义，宁可单列占满宽度。
            **6 栏从 xl 才开始**：1024 时分 6 栏后第二行每张只有 227px，同理。
          */
          <div className="flex flex-col gap-3">
            <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 xl:grid-cols-6">
              {/* ---- 延迟分位：四条线一张图 ---- */}
              <PageSection
                title="延迟分位"
                description={`窗口 ${windowMinutes} 分钟；P50/P95 同轴对比，曲线整体抬高说明上游变慢`}
                bodyClassName="pt-2"
                className="lg:col-span-2 xl:col-span-3"
              >
                {!hasTrend ? (
                  <ChartPlaceholder text="该窗口内没有成功请求" />
                ) : (
                  <TrendChart
                    data={trendRows}
                    xKey="label"
                    height={240}
                    compactAxis={false}
                    series={[
                      {
                        key: "ttftP50",
                        label: "首字 P50",
                        color: "var(--chart-3)",
                        formatValue: (value) => formatMs(value),
                      },
                      {
                        key: "ttftP95",
                        label: "首字 P95",
                        color: "var(--chart-5)",
                        formatValue: (value) => formatMs(value),
                      },
                      {
                        key: "totalP50",
                        label: "总耗时 P50",
                        color: "var(--chart-2)",
                        formatValue: (value) => formatMs(value),
                      },
                      {
                        key: "totalP95",
                        label: "总耗时 P95",
                        color: "var(--chart-1)",
                        formatValue: (value) => formatMs(value),
                      },
                    ]}
                    tooltipFooter={(row) =>
                      `该时段 ${formatCount(Number(row.requests ?? 0))} 次请求`
                    }
                  />
                )}
              </PageSection>

              {/* ---- 输出速度 ---- */}
              <PageSection
                title="输出速度 OTPS"
                description={`token/s，从首字之后算；样本 ${compactNumber(
                  metrics?.speedSample ?? 0,
                )}（≥100 token 才计入）`}
                bodyClassName="pt-2"
                className="lg:col-span-1 xl:col-span-3"
              >
                {!hasTrend ? (
                  <ChartPlaceholder text="该窗口内没有可用样本" />
                ) : (
                  <TrendChart
                    data={trendRows}
                    xKey="label"
                    height={240}
                    compactAxis={false}
                    series={[
                      {
                        key: "otpsP50",
                        label: "P50",
                        color: "var(--chart-2)",
                        formatValue: (value) => `${Math.round(value)} token/s`,
                      },
                      {
                        key: "otpsP95",
                        label: "P95",
                        color: "var(--chart-4)",
                        formatValue: (value) => `${Math.round(value)} token/s`,
                      },
                    ]}
                  />
                )}
              </PageSection>

              {/* ---- 请求结果分布 ---- */}
              <PageSection
                title="请求结果"
                description="成功与失败堆叠；失败柱突然变高时对照上方延迟曲线找关联"
                bodyClassName="pt-2"
                className="lg:col-span-1 xl:col-span-2"
              >
                {!hasTrend ? (
                  <ChartPlaceholder text="该窗口内没有请求" />
                ) : (
                  <TrendChart
                    data={trendRows}
                    xKey="label"
                    height={220}
                    stacked
                    compactAxis={false}
                    series={[
                      {
                        key: "completed",
                        label: "成功",
                        color: "var(--chart-1)",
                        formatValue: (value) => compactNumber(value),
                      },
                      {
                        key: "failed",
                        label: "失败",
                        color: "var(--chart-4)",
                        formatValue: (value) => compactNumber(value),
                      },
                    ]}
                    tooltipFooter={(row) =>
                      `失败率 ${
                        Number(row.requests ?? 0) > 0
                          ? (
                              (Number(row.failed ?? 0) / Number(row.requests ?? 0)) *
                              100
                            ).toFixed(1)
                          : "0.0"
                      }%`
                    }
                  />
                )}
              </PageSection>

              {/* ---- Token 吞吐 ---- */}
              <PageSection
                title="Token 吞吐"
                description={`输入与输出堆叠；缓存命中 ${compactNumber(
                  metrics?.cachedTokens ?? 0,
                )}（占输入 ${cacheRate === null ? "—" : `${cacheRate.toFixed(0)}%`}）`}
                bodyClassName="pt-2"
                className="lg:col-span-1 xl:col-span-2"
              >
                {!hasTrend ? (
                  <ChartPlaceholder text="该窗口内没有 token 消耗" />
                ) : (
                  <TrendChart
                    data={trendRows}
                    xKey="label"
                    height={220}
                    stacked
                    compactAxis={false}
                    series={[
                      {
                        key: "promptTokens",
                        label: "输入",
                        color: "var(--chart-1)",
                        formatValue: (value) => compactNumber(value),
                      },
                      {
                        key: "completionTokens",
                        label: "输出",
                        color: "var(--chart-2)",
                        formatValue: (value) => compactNumber(value),
                      },
                    ]}
                  />
                )}
              </PageSection>

              {/* ---- 运行态 ---- */}
              <PageSection
                title="运行态"
                description="并发水位、账号健康、模型矩阵来源"
                bodyClassName="flex flex-1 flex-col justify-center pt-3"
                className="lg:col-span-1 xl:col-span-2"
              >
                <div className="flex flex-col gap-4">
                  <div className="flex flex-col gap-1.5">
                    <div className="flex items-baseline justify-between text-xs">
                      <span className="text-muted-foreground">全局并发</span>
                      <span className="tabular-nums">
                        {data.concurrency?.used ?? 0} / {data.concurrency?.global ?? 0}
                        <span className="ml-2 text-muted-foreground">
                          {concurrencyPercent}%
                        </span>
                      </span>
                    </div>
                    <div className="h-2 overflow-hidden rounded-full bg-muted">
                      <div
                        className={cn(
                          "h-full rounded-full transition-[width]",
                          concurrencyPercent >= 90
                            ? "bg-destructive"
                            : concurrencyPercent >= 70
                              ? "bg-warning"
                              : "bg-success",
                        )}
                        style={{ width: `${Math.max(concurrencyPercent, 1)}%` }}
                      />
                    </div>
                  </div>

                  <AccountHealthBar accounts={data.accounts} />

                  <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-muted-foreground">
                    <span>
                      账号并发 {data.concurrency?.accountInflight ?? 0}/
                      {data.concurrency?.accountCapacity ?? 0}
                    </span>
                    <span>
                      矩阵 {data.matrix?.live ?? 0} 实时 / {data.matrix?.builtin ?? 0} 内置
                    </span>
                    <span>TTL {data.matrix?.ttlSeconds ?? 0}s</span>
                    <span>
                      网关累计 {formatCount(data.counters?.requests ?? 0)} 请求 · 失败率{" "}
                      {failureRate === null ? "—" : `${failureRate.toFixed(1)}%`}
                    </span>
                    <span className="ml-auto">
                      {new Date(data.checkedAt).toLocaleTimeString("zh-CN")}
                    </span>
                  </div>
                </div>
              </PageSection>
            </div>

            {/* ---- 告警：只能用文字 ---- */}
            {(data.warnings ?? []).length > 0 ? (
              <PageSection
                title="网关告警"
                description="网关自己报告的异常"
                bodyClassName="p-0"
              >
                <ul className="divide-y">
                  {(data.warnings ?? []).map((warning, index) => (
                    <li
                      key={`${index}-${warning.slice(0, 24)}`}
                      className="flex items-start gap-2 px-4 py-2 text-xs"
                    >
                      <AlertTriangle
                        className="mt-0.5 size-3.5 shrink-0 text-warning"
                        aria-hidden="true"
                      />
                      <span className="min-w-0 break-words">{warning}</span>
                    </li>
                  ))}
                </ul>
              </PageSection>
            ) : null}
          </div>
        )}
      </MyBox>
    </div>
  );
};

/**
 * 账号健康堆叠条。
 *
 * 用一条横条按比例切分四类，比四个孤立数字更容易看出"大部分号是不是好的"。
 * 数字仍标在图例里 —— 精确值不能只靠长度估。
 */
const AccountHealthBar = ({
  accounts,
}: {
  accounts?: {
    total: number;
    ready: number;
    cooling: number;
    noCredential: number;
    disabled: number;
  };
}) => {
  const total = accounts?.total ?? 0;

  const segments = [
    { key: "ready", label: "可用", value: accounts?.ready ?? 0, color: "bg-success" },
    { key: "cooling", label: "冷却", value: accounts?.cooling ?? 0, color: "bg-warning" },
    {
      key: "noCredential",
      label: "缺凭据",
      value: accounts?.noCredential ?? 0,
      color: "bg-destructive",
    },
    {
      key: "disabled",
      label: "停用",
      value: accounts?.disabled ?? 0,
      color: "bg-muted-foreground/40",
    },
  ].filter((segment) => segment.value > 0);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between text-xs">
        <span className="text-muted-foreground">账号健康</span>
        <span className="tabular-nums text-muted-foreground">共 {total} 个</span>
      </div>

      {total === 0 ? (
        <div className="h-2 rounded-full bg-muted" />
      ) : (
        <div className="flex h-2 overflow-hidden rounded-full bg-muted">
          {segments.map((segment) => (
            <div
              key={segment.key}
              className={segment.color}
              style={{ width: `${(segment.value / total) * 100}%` }}
              title={`${segment.label} ${segment.value} 个`}
            />
          ))}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
        {segments.map((segment) => (
          <span key={segment.key} className="inline-flex items-center gap-1">
            <span className={cn("size-2 rounded-full", segment.color)} aria-hidden="true" />
            {segment.label} {segment.value}
          </span>
        ))}
      </div>
    </div>
  );
};
