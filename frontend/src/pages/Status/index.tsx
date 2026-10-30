import { Activity, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { listChannelHealth } from "@/api/channels";
import { getGatewayStatus } from "@/api/system";
import type { ChannelHealth, GatewayStatus } from "@/api/types";
import { ChannelHealthCard } from "@/components/common/ChannelHealthCard";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const REFRESH_INTERVAL_MS = 10_000;

/** 统计窗口选项。7 天是默认：够看出趋势，又不会被陈旧数据拖住。 */
const WINDOWS = [
  { days: 1, label: "24 小时" },
  { days: 7, label: "7 天" },
  { days: 30, label: "30 天" },
] as const;

export const StatusPage = () => {
  const [health, setHealth] = useState<ChannelHealth[]>([]);
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [days, setDays] = useState<number>(7);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const [healthResponse, statusResponse] = await Promise.all([
        listChannelHealth(days),
        getGatewayStatus(),
      ]);
      setHealth(healthResponse.data ?? []);
      setStatus(statusResponse.data ?? null);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
      setHealth([]);
      setStatus(null);
    } finally {
      setIsLoading(false);
    }
  }, [days]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  useEffect(() => {
    if (!autoRefresh) {
      return;
    }

    const timer = window.setInterval(() => {
      void load();
    }, REFRESH_INTERVAL_MS);

    return () => window.clearInterval(timer);
  }, [autoRefresh, load]);

  const running = status?.running === true;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-4">
      {/* 工具条：网关状态 + 窗口切换 + 自动刷新 */}
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge
          tone={running ? "success" : status ? "danger" : "neutral"}
          label={running ? "网关运行中" : status ? "网关未运行" : "状态未知"}
        />
        {status?.addr ? (
          <span className="font-mono text-[11px] text-muted-foreground">{status.addr}</span>
        ) : null}

        <div className="ml-auto flex flex-wrap items-center gap-2">
          <div
            className="inline-flex items-center rounded-lg border bg-card p-0.5"
            role="group"
            aria-label="统计窗口"
          >
            {WINDOWS.map((window) => (
              <button
                key={window.days}
                type="button"
                onClick={() => setDays(window.days)}
                aria-pressed={days === window.days}
                className={cn(
                  "rounded-md px-2.5 py-1 text-xs transition-colors",
                  "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                  days === window.days
                    ? "bg-accent font-medium text-accent-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {window.label}
              </button>
            ))}
          </div>

          <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Switch
              checked={autoRefresh}
              onCheckedChange={setAutoRefresh}
              aria-label="自动刷新"
            />
            自动刷新
          </label>

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
          hint="确认 Rust 网关已启动，且 GATEWAY_INTERNAL_TOKEN 与管理面一致。"
        />
      ) : null}

      <MyBox isLoading={isLoading && health.length === 0} text="正在读取分组健康度...">
        {health.length === 0 ? (
          <EmptyState
            icon={Activity}
            title="还没有展示中的分组"
            description="「分组状态」只显示在分组管理里开启了展示的分组。去「分组管理 → 编辑」打开「分组状态」，并选好探活模型。"
            action={
              <Button size="sm" asChild>
                <Link to="/channels">去分组管理</Link>
              </Button>
            }
          />
        ) : (
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
            {health.map((channel) => (
              <ChannelHealthCard key={channel.id} channel={channel} days={days} />
            ))}
          </div>
        )}
      </MyBox>

      {status && status.warnings.length > 0 ? (
        <PageSection title="告警" description="网关启动与配置检查中的问题">
          <ul className="space-y-1 text-xs text-warning">
            {status.warnings.map((warning) => (
              <li key={warning} className="break-words">
                {warning}
              </li>
            ))}
          </ul>
        </PageSection>
      ) : null}

      {/*
        这里**不再有**探活模型设置入口。

        它已经挪到「分组管理 → 编辑分组」：展示开关和探活模型是同一块设置，
        而藏起来的分组在这张页面根本没有卡片 —— 入口放在这里的话，
        一旦关闭展示就再也改不回来了。
      */}
    </div>
  );
};
