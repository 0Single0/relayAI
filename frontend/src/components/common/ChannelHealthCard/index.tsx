import { Clock3, Gauge, PencilLine, Radio, Timer } from "lucide-react";
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
 * 把探活时刻转成"几分钟前"。
 *
 * 卡片上要回答的是"这个绿灯有多新"，绝对时间需要心算，
 * 所以主文案用相对时间，绝对时间放在 title 里备查。
 */
const formatAgo = (iso: string | null) => {
  if (!iso) {
    return "还未探活";
  }
  const timestamp = Date.parse(iso);
  if (Number.isNaN(timestamp)) {
    return "还未探活";
  }
  const minutes = Math.floor((Date.now() - timestamp) / 60_000);
  if (minutes < 1) {
    return "刚刚";
  }
  if (minutes < 60) {
    return `${minutes} 分钟前`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours} 小时前`;
  }
  return `${Math.floor(hours / 24)} 天前`;
};

/**
 * 分组健康度判定。
 *
 * 顺序很重要：**先看有没有探过**。没有探活记录的分组成功率是 null，
 * 不能判成"异常"——没配探活模型的分组本来就不会有记录，
 * 把它标红会让告警失去意义。
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
  if (channel.probes === 0 || channel.successRate === null) {
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
  title,
}: {
  icon: typeof Timer;
  label: string;
  value: string;
  title?: string;
}) => (
  <div className="flex min-w-0 flex-col gap-0.5" title={title}>
    <span className="flex items-center gap-1 text-[11px] text-muted-foreground">
      <Icon className="size-3" aria-hidden="true" />
      {label}
    </span>
    <span className="truncate text-sm font-medium tabular-nums">{value}</span>
  </div>
);

export type ChannelHealthCardProps = {
  channel: ChannelHealth;
  /** 统计窗口天数，用于"探活可用性 · 7 天"这类文案。 */
  days: number;
  /**
   * 打开「设置探活模型」。
   *
   * 不传则不显示那个铅笔按钮 —— 只有管理员该看到它。
   */
  onEditProbeModel?: (channel: ChannelHealth) => void;
};

/**
 * 分组状态卡片。
 *
 * 信息层级（自上而下）：身份 → 状态判定 → 探活指标 → 探活可用性 → 最近探活。
 * 这样扫视一张卡片时先回答"它能用吗"，再回答"探得怎么样"，最后才是"刚才稳不稳"。
 *
 * 全部数字都来自定时探活，与真实请求用量无关 —— 这一点必须在文案上说清，
 * 否则"999 次探活"会被读成"999 次调用"。
 */
export const ChannelHealthCard = ({
  channel,
  days,
  onEditProbeModel,
}: ChannelHealthCardProps) => {
  const health = healthOf(channel);
  /*
    模型列表优先用 probeModels；老后端只回 probeModel 时退化成单元素，
    免得整行显示成"自动"而实际正在探某个模型。
  */
  const probeModels =
    channel.probeModels.length > 0
      ? channel.probeModels
      : channel.probeModel
        ? [channel.probeModel]
        : [];

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
          <p className="mt-1 flex items-start gap-1 text-[11px] text-muted-foreground">
            <Gauge className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
            <span className="shrink-0">探活模型</span>
            <span
              className={cn(
                "min-w-0 break-all font-mono",
                probeModels.length > 0 ? "text-foreground" : "text-muted-foreground",
              )}
              title={
                probeModels.length > 0
                  ? `每轮探 ${probeModels.length} 个模型：${probeModels.join("、")}`
                  : "未指定：由网关按「免费优先」自动挑一个属于该分组的模型"
              }
            >
              {probeModels.length > 0 ? probeModels.join("、") : "自动（免费优先）"}
            </span>
            {onEditProbeModel ? (
              <button
                type="button"
                onClick={() => onEditProbeModel(channel)}
                aria-label={`设置 ${channel.name} 的探活模型`}
                title="更换探活模型"
                className="ml-auto inline-flex shrink-0 cursor-pointer items-center rounded px-1 py-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none"
              >
                <PencilLine className="size-3" aria-hidden="true" />
              </button>
            ) : null}
          </p>
        </div>
        <StatusBadge tone={health.tone} label={health.label} />
      </header>

      {/* 探活指标：耗时与新鲜度 */}
      <div className="grid grid-cols-2 gap-2 rounded-lg bg-muted/50 px-3 py-2">
        <Metric
          icon={Clock3}
          label="探活耗时"
          value={formatMs(channel.avgLatencyMs)}
          title="窗口内成功探活的平均端到端耗时（不含 0 毫秒的失败记录）"
        />
        <Metric
          icon={Timer}
          label="最近探活"
          value={formatAgo(channel.lastProbeAt)}
          title={channel.lastProbeAt ?? "还没有任何探活记录"}
        />
      </div>

      {/* 探活可用性：口径与下方状态条同源 */}
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-xs text-muted-foreground">
          探活可用性 · {days} 天
          <span className="ml-1 text-[11px]">（{channel.probes} 次）</span>
        </span>
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
        最近探活的细条（一格 = 一次探活）。

        保留它是因为百分比是窗口聚合，一次刚发生的故障在百分比上看不出来；
        细条一眼能看出"刚才有没有连续失败"。而"一格 = 一次探活"意味着
        它的增长速度只取决于探活间隔，不会因为业务流量变大而变快。
      */}
      {channel.recent.length > 0 ? (
        <StatusStrip statuses={channel.recent} />
      ) : (
        <p className="rounded-md bg-muted/50 px-2 py-1.5 text-[11px] leading-4 text-muted-foreground">
          <Radio className="mr-1 inline size-3" aria-hidden="true" />
          还没有探活记录：先给这个分组选一个探活模型，定时探活下一轮就会有数据。
        </p>
      )}

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
