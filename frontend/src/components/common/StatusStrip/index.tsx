import { cn } from "@/lib/utils";

export type StatusStripProps = {
  /** 旧 → 新的状态序列，值原样来自 `usage_logs.status`。 */
  statuses: string[];
  /** 期望的格数；不足的用空格补齐（左侧留空 = 那段时间没有请求）。 */
  slots?: number;
  className?: string;
  /** 屏幕阅读器用的说明前缀。 */
  label?: string;
};

/**
 * 把 `usage_logs.status` 映射成三档颜色。
 *
 * 为什么不区分 `client_aborted` / `upstream_aborted`：细条只有 2px 宽，
 * 四种颜色的差别在视觉上根本分辨不出来，反而会让人以为颜色有别的含义。
 * 只有"成功 / 未完成 / 失败"三档是一眼可读的。
 */
const toneOf = (status: string) => {
  if (status === "completed") {
    return "bg-success/70";
  }
  if (status === "client_aborted" || status === "upstream_aborted") {
    return "bg-warning/70";
  }
  return "bg-destructive/70";
};

const toneLabel = (status: string) => {
  if (status === "completed") {
    return "成功";
  }
  if (status === "client_aborted" || status === "upstream_aborted") {
    return "中断";
  }
  return "失败";
};

/**
 * 最近 N 次请求的状态细条（旧 → 新）。
 *
 * 数据不足时**从左侧留空**而不是把已有格子拉伸：拉伸会让"只跑过 2 次"
 * 看起来和"跑满 60 次"一样密，那是在骗人。留空清楚地表达了样本量。
 */
export const StatusStrip = ({
  statuses,
  slots = 60,
  className,
  label = "最近请求状态",
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
      ? "还没有请求记录"
      : `最近 ${statuses.length} 次请求：${Object.entries(counts)
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
