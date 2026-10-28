import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export type StatusTone = "neutral" | "success" | "warning" | "danger" | "info";

const toneClass: Record<StatusTone, string> = {
  neutral: "border-border bg-muted/60 text-muted-foreground",
  success: "border-success/30 bg-success/10 text-success",
  warning: "border-warning/30 bg-warning/10 text-warning",
  danger: "border-destructive/30 bg-destructive/10 text-destructive",
  info: "border-primary/30 bg-primary/10 text-primary",
};

const dotClass: Record<StatusTone, string> = {
  neutral: "bg-muted-foreground/60",
  success: "bg-success",
  warning: "bg-warning",
  danger: "bg-destructive",
  info: "bg-primary",
};

export type StatusBadgeProps = {
  tone: StatusTone;
  label: string;
  icon?: LucideIcon;
  /** 是否显示前置圆点。 */
  dot?: boolean;
  className?: string;
};

/** 状态徽标。颜色只来自主题 token，页面里不得写死色值。 */
export const StatusBadge = ({
  tone,
  label,
  icon: Icon,
  dot = true,
  className,
}: StatusBadgeProps) => (
  <span
    className={cn(
      "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] leading-4 whitespace-nowrap",
      toneClass[tone],
      className,
    )}
  >
    {Icon ? (
      <Icon className="size-3" aria-hidden="true" />
    ) : dot ? (
      <span
        className={cn("size-1.5 rounded-full", dotClass[tone])}
        aria-hidden="true"
      />
    ) : null}
    {label}
  </span>
);

export type PageSectionProps = {
  title: string;
  description?: string;
  /** 右上角动作区。 */
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClassName?: string;
};

/**
 * 页面内的区块容器。
 *
 * 刻意**不用卡片套卡片**（AGENTS.md 禁止嵌套卡片）：这里是最外层区块，
 * 内部列表/表格用描边分隔而不是再包一层。
 */
export const PageSection = ({
  title,
  description,
  actions,
  children,
  className,
  bodyClassName,
}: PageSectionProps) => (
  <section
    className={cn(
      // 与 StatCard 用同一档 surface 阴影：上下两组卡片必须看起来是同一个系统，
      // 否则"上面的卡片有阴影、下面的没有"会让人以为是两种组件。
      "flex min-h-0 min-w-0 flex-col rounded-xl border bg-card",
      "shadow-[var(--shadow-surface)]",
      className,
    )}
  >
    <header className="flex flex-wrap items-start justify-between gap-2 border-b px-4 py-3">
      <div className="min-w-0">
        <h2 className="text-sm font-medium">{title}</h2>
        {description ? (
          <p className="mt-0.5 text-xs leading-5 text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {actions ? (
        <div className="flex shrink-0 items-center gap-2">{actions}</div>
      ) : null}
    </header>
    <div className={cn("min-h-0 flex-1 p-4", bodyClassName)}>{children}</div>
  </section>
);
