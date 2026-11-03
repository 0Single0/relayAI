import { cn } from "@/lib/utils";
import { probeStatusClassOf } from "@/lib/channelHealth";

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
 * 为什么不区分 `auth` / `quota` / `error`：细条只有几像素宽，
 * 几种红色的差别在视觉上根本分辨不出来，反而会让人以为颜色有别的含义。
 * 只有"通过 / 限流 / 失败"三档是一眼可读的，具体原因在卡片底部的文字里。
 *
 * 具体色值现在来自 `lib/channelHealth.ts` 的 11 档色阶（对齐 sub2api），
 * 这样细条、百分比文字、图例用的是同一套颜色，不会各说各话。
 */
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
 * 最近 N 次探活的状态细条（旧 → 新，右侧 = 现在）。
 *
 * **一格 = 一次探活**，不是一次真实请求。这是关键的口径：
 * 探活按设置的间隔（最低 5 分钟）跑，所以这排格子只会按那个节奏增长；
 * 如果它涨得很快，说明探活间隔调短了或手动探活点得勤，而不是流量在涨。
 *
 * 数据不足时**从左侧留空**而不是把已有格子拉伸：拉伸会让"只探过 2 次"
 * 看起来和"探满 60 次"一样密，那是在骗人。留空清楚地表达了样本量。
 *
 * 细条高度从 24px 抬到 28px，并在下方补一行 PAST / NOW 轴标（sub2api 的做法）。
 *
 * 为什么值得抬：一格只有 2px 宽，24px 高的排条读起来像一条"色带"，
 * 而"刚才有没有抖"恰恰要靠右端那几格区分出来；抬到 28px 后
 * 失败与通过的边界一眼可辨。轴标则回答另一个必要的问题 ——
 * 最右边那一格是"刚才"，不是"上一次探活"。
 *
 * 为什么不用 Recharts：这里没有坐标轴、没有数值、没有刻度，每个数据点
 * 只有"通过/限流/失败"三态，画成图表反而要伪造一条无量纲的 Y 轴。
 * 它是**状态指示条**（同一口径的参照物是蜂窝信号格、uptime 心跳条），
 * 用 `flex` + 固定的格子宽度才是它该有的样子；项目里"图表必须走 Recharts"
 * 那条约束针对的是带数值与坐标轴的趋势图，不是这种三态指示条。
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
    <div className={cn("flex flex-col gap-1", className)}>
      <div
        className="flex h-7 items-stretch gap-[2px]"
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
            className={cn("min-w-px flex-1 rounded-[1px]", probeStatusClassOf(status))}
            aria-hidden="true"
          />
        ))}
      </div>
      {/* 时间轴方向：左旧右新。没有它，"右端那几格红了"读不出有多新。 */}
      <div
        className="flex items-center justify-between text-[9px] leading-3 font-medium tracking-widest text-muted-foreground/70 uppercase"
        aria-hidden="true"
      >
        <span>Past</span>
        <span>Now</span>
      </div>
    </div>
  );
};
