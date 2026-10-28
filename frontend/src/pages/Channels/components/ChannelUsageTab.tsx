import { useCallback, useEffect, useState } from "react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { BarChart3 } from "lucide-react";
import { getUsageSeries, getUsageSummary } from "@/api/usage";
import type { UsageSeriesPoint, UsageSummary } from "@/api/types";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { getRequestErrorMessage } from "@/utils/request";

const emptySummary: UsageSummary = {
  requests: 0,
  successRate: 0,
  promptTokens: 0,
  completionTokens: 0,
  totalTokens: 0,
  cachedTokens: 0,
  upstreamCost: 0,
  billedCost: 0,
  avgTtftMs: null,
};

type ChannelUsageTabProps = {
  channelId: string;
};

export const ChannelUsageTab = ({ channelId }: ChannelUsageTabProps) => {
  const [summary, setSummary] = useState<UsageSummary>(emptySummary);
  const [series, setSeries] = useState<UsageSeriesPoint[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    const now = new Date();
    const from = new Date(now.getTime() - 7 * 24 * 3600 * 1000);

    try {
      const [summaryResponse, seriesResponse] = await Promise.all([
        getUsageSummary({
          from: from.toISOString(),
          to: now.toISOString(),
          groupBy: "channel",
        }),
        getUsageSeries({
          from: from.toISOString(),
          to: now.toISOString(),
          bucket: "day",
        }),
      ]);

      const bucket = summaryResponse.data?.buckets.find(
        (entry) => entry.key === channelId,
      );

      setSummary(
        bucket
          ? {
              ...emptySummary,
              requests: bucket.requests,
              totalTokens: bucket.totalTokens,
              upstreamCost: bucket.upstreamCost,
              billedCost: bucket.billedCost,
              successRate: bucket.successRate,
            }
          : emptySummary,
      );
      setSeries(seriesResponse.data ?? []);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, [channelId]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const chartData = series.map((point) => ({
    ...point,
    label: new Date(point.bucket).toLocaleDateString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
    }),
  }));

  return (
    <div className="flex flex-col gap-3">
      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox isLoading={isLoading && series.length === 0} text="正在加载用量...">
        <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
          {[
            { label: "近 7 天请求", value: summary.requests.toLocaleString() },
            {
              label: "Token",
              value: summary.totalTokens.toLocaleString(),
            },
            {
              label: "记账成本",
              value: `$${summary.billedCost.toFixed(4)}`,
            },
            {
              label: "成功率",
              value: `${(summary.successRate * 100).toFixed(1)}%`,
            },
          ].map((item) => (
            <div key={item.label} className="rounded-lg border bg-card p-3">
              <p className="text-[11px] text-muted-foreground">{item.label}</p>
              <p className="mt-0.5 text-lg font-semibold tabular-nums">
                {item.value}
              </p>
            </div>
          ))}
        </div>

        {chartData.length === 0 ? (
          <EmptyState
            icon={BarChart3}
            title="暂无用量数据"
            description="该渠道近 7 天还没有请求记录。"
          />
        ) : (
          <div className="mt-3 h-48 w-full rounded-lg border bg-card p-2">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={chartData} margin={{ top: 8, right: 8, bottom: 0, left: -20 }}>
                <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
                <XAxis
                  dataKey="label"
                  tick={{ fontSize: 11, fill: "var(--muted-foreground)" }}
                  stroke="var(--border)"
                />
                <YAxis
                  tick={{ fontSize: 11, fill: "var(--muted-foreground)" }}
                  stroke="var(--border)"
                />
                <Tooltip
                  contentStyle={{
                    background: "var(--popover)",
                    border: "1px solid var(--border)",
                    borderRadius: 8,
                    fontSize: 12,
                  }}
                />
                <Bar dataKey="requests" fill="var(--chart-1)" radius={[3, 3, 0, 0]} />
              </BarChart>
            </ResponsiveContainer>
          </div>
        )}
      </MyBox>
    </div>
  );
};
