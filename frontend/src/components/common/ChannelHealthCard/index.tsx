import { Clock3, Gauge, PencilLine, Radio, Zap } from "lucide-react";
import { Link } from "react-router-dom";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";
import { StatusStrip } from "@/components/common/StatusStrip";
import type { ChannelHealth } from "@/api/types";
import { cn } from "@/lib/utils";

const ADAPTER_LABEL: Record<string, string> = {
  workbuddy: "WorkBuddy",
  trae: "Trae",
  custom: "自定义供应商",
  fake: "链路自测",
};

const formatMs = (value: number | null) =>
  value === null ? "—" : value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${value}ms`;

const formatPercent = (value: number | null) =>
  value === null ? "—" : `${(value * 100).toFixed(2)}%`;

/**
 * 渠道健康度判定。
 *
 * 顺序很重要：**先看有没有测过**。没有任何请求的渠道成功率是 null，
 * 不能判成"异常"——新品还没接入就被标红，会让告警失去意义。
 */
const healthOf = (channel: ChannelHealth): { tone: StatusTone; label: string } => {
  if (!channel.enabled) {
    return { tone: "neutral", label: "已停用" };
  }
  if (channel.readyAccounts === null) {
    return { tone: "neutral", label: "网关离线" };
  }
  if (channel.readyAccounts === 0) {
    return { tone: "danger", label: "无可用账号" };
  }
  if (channel.requests === 0 || channel.successRate === null) {
    return { tone: "info", label: "待观测" };
  }
  if (channel.successRate < 0.9) {
    return { tone: "danger", label: "降级" };
  }
  if (channel.successRate < 0.99) {
    return { tone: "warning", label: "波动" };
  }
  return { tone: "success", label: "正常" };
};

const Metric = ({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof Zap;
  label: string;
  value: string;
}) => (
  <div className="flex min-w-0 flex-col gap-0.5">
    <span className="flex items-center gap-1 text-[11px] text-muted-foreground">
      <Icon className="size-3" aria-hidden="true" />
      {label}
    </span>
    <span className="truncate text-sm font-medium tabular-nums">{value}</span>
  </div>
);

export type ChannelHealthCardProps = {
  channel: ChannelHealth;
  /** 统计窗口天数，用于"可用性 · 7 天"这类文案。 */
  days: number;
  /**
   * 打开「设置探活模型」。
   *
   * 不传则不显示那个铅笔按钮 —— 只有管理员该看到它。
   */
  onEditProbeModel?: (channel: ChannelHealth) => void;
};

/**
 * 渠道状态卡片。
 *
 * 信息层级（自上而下）：身份 → 状态判定 → 性能指标 → 可用性 → 最近请求。
 * 这样扫视一张卡片时先回答"它能用吗"，再回答"好用吗"，最后才是"刚才稳不稳"。
 */
export const ChannelHealthCard = ({
  channel,
  days,
  onEditProbeModel,
}: ChannelHealthCardProps) => {
  const health = healthOf(channel);

  return (
    <article
      className={cn(
        "flex min-w-0 flex-col gap-3 rounded-xl border bg-card p-4",
        /*
          浅阴影（与模型广场同一档 `--shadow-card`）：一屏常有三到六张卡，
          阴影只负责让卡片边缘不像贴纸一样平，不做"浮起的卡片墙"。
          hover 时加深一档，配合描边变化提示"这块可点"。
        */
        "shadow-[var(--shadow-card)] transition-[box-shadow,border-color]",
        "hover:border-border-strong hover:shadow-[var(--shadow-card-hover)]",
      )}
    >
      {/* 身份 + 状态。标题可点：底部"详情"按钮已去掉，入口挪到这里。 */}
      <header className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <Link
            to={`/channels/${encodeURIComponent(channel.id)}`}
            className="block truncate text-sm font-medium hover:text-primary"
          >
            {channel.name}
          </Link>
          <p className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
            <span>{ADAPTER_LABEL[channel.adapter] ?? channel.adapter}</span>
            <span aria-hidden="true">·</span>
            <span className="truncate font-mono">{channel.id}</span>
          </p>
          {/*
            探活模型：让人知道"这个绿灯是用哪个模型测出来的"。
            没指定时是网关自动挑（免费优先），显示成"自动"而不是留空 ——
            留空会被读成"没有探活"。
          */}
          <p className="mt-1 flex items-center gap-1 text-[11px] text-muted-foreground">
            <Gauge className="size-3 shrink-0" aria-hidden="true" />
            <span className="shrink-0">探活模型</span>
            <span
              className={cn(
                "truncate font-mono",
                channel.probeModel ? "text-foreground" : "text-muted-foreground",
              )}
              title={
                channel.probeModel
                  ? `用 ${channel.probeModel} 探活`
                  : "未指定：由网关按「免费优先」自动挑一个属于该分组的模型"
              }
            >
              {channel.probeModel || "自动（免费优先）"}
            </span>
            {onEditProbeModel ? (
              <button
                type="button"
                onClick={() => onEditProbeModel(channel)}
                aria-label={`设置 ${channel.name} 的探活模型`}
                title="更换探活模型"
                className="ml-auto inline-flex cursor-pointer items-center rounded px-1 py-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
              >
                <PencilLine className="size-3" aria-hidden="true" />
              </button>
            ) : null}
          </p>
        </div>
        <StatusBadge tone={health.tone} label={health.label} />
      </header>

      {/* 性能指标 */}
      <div className="grid grid-cols-2 gap-2 rounded-lg bg-muted/50 px-3 py-2">
        <Metric
          icon={Clock3}
          label="首字延迟"
          value={formatMs(channel.avgTtftMs)}
        />
        <Metric icon={Zap} label="平均耗时" value={formatMs(channel.avgLatencyMs)} />
      </div>

      {/* 可用性 */}
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-xs text-muted-foreground">可用性 · {days} 天</span>
        <span
          className={cn(
            "text-2xl leading-8 font-semibold tabular-nums",
            channel.successRate === null
              ? "text-muted-foreground"
              : channel.successRate >= 0.99
                ? "text-success"
                : channel.successRate >= 0.9
                  ? "text-warning"
                  : "text-destructive",
          )}
        >
          {formatPercent(channel.successRate)}
        </span>
      </div>

      {/*
        最近请求的状态条（只画格子，不显示 token / 次数汇总）。

        保留状态条是因为它一眼能看出"刚才有没有连续失败"——
        可用性百分比是 7 天聚合，一次刚发生的故障在百分比上看不出来。
        但**不显示 "N token · M 次"**：那个数字混着历史导入与系统自身流量，
        既不反映真实业务量，也会让人以为渠道在猛跑。
      */}
      {channel.recent.length > 0 ? (
        <StatusStrip statuses={channel.recent} />
      ) : null}

      {/*
        底部账号行已移除：`N 个账号 · 运行态未知` 对使用者没有决策价值
        （"运行态未知"只说明网关没上报，改不了任何事），
        而"详情"入口在卡片标题上点一下也一样能进。
      */}

      {channel.lastError ? (
        <p
          className="line-clamp-2 rounded-md bg-destructive/5 px-2 py-1.5 text-[11px] leading-4 text-destructive"
          title={channel.lastError}
        >
          <Radio className="mr-1 inline size-3" aria-hidden="true" />
          {channel.lastError}
        </p>
      ) : null}
    </article>
  );
};
