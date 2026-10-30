import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Sparkline } from "@/components/common/Charts/Sparkline";
import { cn } from "@/lib/utils";

export type StatCardTone = "primary" | "violet" | "amber" | "green" | "neutral";

/**
 * 图标块的配色。
 *
 * 只动图标底色与图标本身，不动数值或卡片背景：数值必须保持高对比度，
 * 整张卡染成彩色会让一屏八张卡变成调色盘，反而看不出哪个重要。
 */
const toneClass: Record<StatCardTone, string> = {
  primary: "bg-primary/10 text-primary",
  violet: "bg-[color-mix(in_oklab,var(--chart-2)_14%,transparent)] text-[var(--chart-2)]",
  amber: "bg-[color-mix(in_oklab,var(--chart-3)_16%,transparent)] text-[var(--chart-3)]",
  green: "bg-success/10 text-success",
  neutral: "bg-muted text-muted-foreground",
};

const sparkColor: Record<StatCardTone, string> = {
  primary: "var(--chart-1)",
  violet: "var(--chart-2)",
  amber: "var(--chart-3)",
  green: "var(--chart-4)",
  neutral: "var(--muted-foreground)",
};

export type StatCardProps = {
  label: string;
  /** 主数值。已经是格式化后的字符串，格式化属于调用方（单位不同）。 */
  value: string;
  /** 单位后缀，小字跟在数值后面。 */
  unit?: string;
  /** 一行补充说明。 */
  hint?: ReactNode;
  icon?: LucideIcon;
  /** 图标块配色。 */
  tone?: StatCardTone;
  /** 相对上一个周期的变化百分比；null / undefined 表示不显示。 */
  trend?: number | null;
  /** 趋势的语义：涨是好还是坏。流量涨是好事，失败率涨是坏事。 */
  trendTone?: "positive" | "negative";
  /** 趋势标签（"较昨日" / "较上周"）。 */
  trendLabel?: string;
  /** 右侧迷你曲线数据。 */
  spark?: number[];
  className?: string;
};

/**
 * 指标卡。
 *
 * 版式（对齐参考图）：左侧图标块 + 标签 / 大数值 / 变化说明，
 * 右侧一条迷你曲线。两者用 `flex` 分栏、曲线固定宽度，
 * 这样同一排的卡片数值基线一定对齐，不会因为标签长短而错位。
 *
 * 阴影用 `--shadow-2`（很轻的一档）：卡片需要从背景"浮"起来一点点，
 * 但仪表盘上同时有八张卡，阴影再重就变成八块积木。
 */
export const StatCard = ({
  label,
  value,
  unit,
  hint,
  icon: Icon,
  tone = "primary",
  trend,
  trendTone = "positive",
  trendLabel,
  spark,
  className,
}: StatCardProps) => {
  const showTrend = typeof trend === "number" && Number.isFinite(trend);
  const isGood = trendTone === "positive" ? (trend ?? 0) >= 0 : (trend ?? 0) < 0;

  return (
    <div
      className={cn(
        "flex min-w-0 items-stretch gap-3 rounded-xl border bg-card p-4",
        "shadow-[var(--shadow-surface)] transition-shadow hover:shadow-[var(--shadow-surface-hover)]",
        className,
      )}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-center gap-2">
          {Icon ? (
            <span
              className={cn(
                "flex size-6 shrink-0 items-center justify-center rounded-md",
                toneClass[tone],
              )}
            >
              <Icon className="size-3.5" aria-hidden="true" />
            </span>
          ) : null}
          <span className="min-w-0 truncate text-xs text-muted-foreground">{label}</span>
        </div>

        <div className="flex flex-wrap items-baseline gap-1">
          <span className="text-[26px] leading-8 font-semibold tracking-tight tabular-nums">
            {value}
          </span>
          {unit ? (
            <span className="text-xs text-muted-foreground">{unit}</span>
          ) : null}
        </div>

        <div className="flex min-w-0 items-center gap-1 text-[11px] leading-4">
          {showTrend ? (
            <span
              className={cn(
                "shrink-0 tabular-nums",
                isGood ? "text-success" : "text-destructive",
              )}
            >
              {trend >= 0 ? "↑" : "↓"} {Math.abs(trend).toFixed(1)}%
            </span>
          ) : null}
          {hint ? (
            <span className="min-w-0 truncate text-muted-foreground">{hint}</span>
          ) : null}
        </div>
      </div>

      {/*
        曲线固定宽度但收窄到 14：一行四张卡时单卡只有约 230px，
        原来的 20/24 会把左侧数值区压到 100px 以下，「平均缓存命中率」
        这种 7 个字的标签和补充说明就只剩省略号了。
      */}
      {spark && spark.length > 0 ? (
        <div className="flex w-14 shrink-0 flex-col justify-center">
          <Sparkline
            values={spark}
            color={sparkColor[tone]}
            height={36}
            label={`${label}趋势`}
          />
          {trendLabel ? (
            <span className="mt-0.5 text-right text-[10px] text-muted-foreground">
              {trendLabel}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
};
