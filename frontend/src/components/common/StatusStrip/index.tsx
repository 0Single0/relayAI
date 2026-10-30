import { cn } from "@/lib/utils";

export type StatusStripProps = {
  /**
   * 旧 → 新的状态序列，值原样来自 `channel_probes.status`
   * （`ok` / `limited` / `auth` / `quota` / `error`）。
   */
  statuses: string[];
  /** 期望的格数；不足的用空格补齐（左侧留空 = 那段时间还没探过）。 */
  slots?: number;
  className?: string;
  /** 屏幕阅读器用的说明前缀。 */
  label?: string;
};

/**
 * 把 `channel_probes.status` 映射成三档颜色。
 *
 * 为什么不区分 `auth` / `quota` / `error`：细条只有 2px 宽，
 * 几种颜色的差别在视觉上根本分辨不出来，反而会让人以为颜色有别的含义。
 * 只有"通过 / 限流 / 失败"三档是一眼可读的，具体原因在卡片底部的文字里。
 */
const toneOf = (status: string) => {
  if (status === "ok") {
    return "bg-success/70";
  }
  if (status === "limited") {
    return "bg-warning/70";
  }
  return "bg-destructive/70";
};

const toneLabel = (status: string) => {
  if (status === "ok") {
    return "通过";
  }
  if (status === "limited") {
    return "限流";
  }
  return "失败";
};

/**
 * 最近 N 次探活的状态细条（旧 → 新）。
 *
 * **一格 = 一次探活**，不是一次真实请求。这是关键的口径：
 * 探活按设置的间隔（最低 5 分钟）跑，所以这排格子只会按那个节奏增长；
 * 如果它涨得很快，说明探活间隔调短了或手动探活点得勤，而不是流量在涨。
 *
 * 数据不足时**从左侧留空**而不是把已有格子拉伸：拉伸会让"只探过 2 次"
 * 看起来和"探满 60 次"一样密，那是在骗人。留空清楚地表达了样本量。
 */
export const StatusStrip = ({
  statuses,
  slots = 60,
  className,
  label = "最近探活状态",
}: StatusStripProps) => {
  const total = Math.max(slots, statuses.length);
  const padding = Math.max(0, total - statuses.length);
  const counts = statuses.reduce<Record<string, number>>((acc, status) => {
    const key = toneLabel(status);
    acc[key] = (acc[key] ?? 0) + 1;
    return acc;
  }, {});

  const summary =
    statuses.length === 0
      ? "还没有探活记录"
      : `最近 ${statuses.length} 次探活：${Object.entries(counts)
          .map(([key, value]) => `${key} ${value}`)
          .join("，")}`;

  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <div
        className="flex h-6 items-stretch gap-[2px]"
        role="img"
        aria-label={`${label}。${summary}`}
      >
        {Array.from({ length: padding }, (_, index) => (
          <span
            key={`pad-${index}`}
            className="min-w-px flex-1 rounded-[1px] bg-muted"
            aria-hidden="true"
          />
        ))}
        {statuses.map((status, index) => (
          <span
            key={`${status}-${index}`}
            className={cn("min-w-px flex-1 rounded-[1px]", toneOf(status))}
            aria-hidden="true"
          />
        ))}
      </div>
    </div>
  );
};
