import { cn } from "@/lib/utils";

export type QuotaBarProps = {
  /** 剩余量。null 表示上游没给这个口径。 */
  remaining: number | null;
  /** 周期总量。null 表示上游没给（只有剩余量，画不出比例）。 */
  total: number | null;
  /** 口径名称，如「积分」。 */
  label?: string;
  unit?: string;
  className?: string;
};

const formatNumber = (value: number) => {
  if (value >= 10000) {
    return `${(value / 1000).toFixed(1)}k`;
  }
  return Number.isInteger(value) ? String(value) : value.toFixed(1);
};

/**
 * 额度进度条：显示**剩余 / 总量**，条本身表示"还剩多少"。
 *
 * 为什么用"剩余"而不是"已用"填条：
 * 这一列回答的问题是"这个号还能撑多久"，所以条越长 = 越健康，一眼就能筛出快用完的号。
 * 用"已用"填条的话，条越长越危险，颜色和直觉相反。
 *
 * 阈值：剩余 ≤ 20% 转黄，≤ 10% 转红。这两个数是运维经验值 ——
 * 20% 时还来得及补号，10% 时通常只剩几小时。
 *
 * `total` 缺失时退化成纯文字（只有剩余量），**不画 0% 或 100% 的条**：
 * 那会让人以为"快用完了"或"满的"，而实际上只是不知道总量。
 */
export const QuotaBar = ({
  remaining,
  total,
  label,
  unit,
  className,
}: QuotaBarProps) => {
  if (remaining === null) {
    return <span className={cn("text-xs text-muted-foreground", className)}>—</span>;
  }

  const suffix = unit ? ` ${unit}` : "";

  // 只有剩余量、没有总量：画不出比例，老实显示数字。
  if (total === null || total <= 0) {
    return (
      <div className={cn("flex flex-col items-center gap-0.5", className)}>
        <span className="text-xs tabular-nums">{formatNumber(remaining)}</span>
        {label ? (
          <span className="text-[11px] text-muted-foreground">{label}</span>
        ) : null}
      </div>
    );
  }

  const ratio = Math.max(0, Math.min(1, remaining / total));
  const percent = Math.round(ratio * 100);

  const tone =
    ratio <= 0.1
      ? { bar: "bg-destructive", text: "text-destructive" }
      : ratio <= 0.2
        ? { bar: "bg-warning", text: "text-warning" }
        : { bar: "bg-success", text: "text-muted-foreground" };

  return (
    <div className={cn("mx-auto flex w-full max-w-[132px] flex-col items-center gap-1", className)}>
      {/* `whitespace-nowrap`：剩余/总量一旦折行，"7219.1" 与 "/ 10.3k" 就分家了。 */}
      <div className="flex w-full items-baseline justify-center gap-1 text-xs whitespace-nowrap tabular-nums">
        <span className={cn("font-medium", tone.text)}>{formatNumber(remaining)}</span>
        <span className="text-muted-foreground">/ {formatNumber(total)}</span>
      </div>

      <div
        role="progressbar"
        aria-valuenow={percent}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`剩余${label ?? "额度"} ${formatNumber(remaining)}${suffix}，共 ${formatNumber(total)}`}
        className="h-1 w-full overflow-hidden rounded-full bg-muted"
      >
        <div
          className={cn("h-full rounded-full transition-[width]", tone.bar)}
          style={{ width: `${percent}%` }}
        />
      </div>
    </div>
  );
};
