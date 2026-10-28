import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export type RowAction = {
  /** 无障碍名称，同时作为原生 tooltip。图标按钮**必须**有它。 */
  label: string;
  /**
   * 稳定标识，用于 React key。
   *
   * 不能用 `label` 当 key：同一行里可能有多个动作共享同一段文案
   * （例如两把动作都因为"明文已丢失"而改名），那会触发
   * "Encountered two children with the same key"，React 会漏渲染或错位。
   */
  id: string;
  icon: LucideIcon;
  onClick: () => void;
  /** 语义色：不同动作给不同悬停底色。 */
  tone?: "default" | "primary" | "success" | "warning" | "danger";
  disabled?: boolean;
};

const toneClass: Record<NonNullable<RowAction["tone"]>, string> = {
  default: "hover:bg-muted hover:text-foreground",
  primary: "hover:bg-primary/10 hover:text-primary",
  success: "hover:bg-success/10 hover:text-success",
  warning: "hover:bg-warning/10 hover:text-warning",
  danger: "hover:bg-destructive/10 hover:text-destructive",
};

/**
 * 表格行操作：**纯图标，不带文字**。
 *
 * 为什么纯图标：一行里塞"编辑 / 详情 / 禁用 / 删除"四个词，操作列会宽到
 * 把数据列挤扁。文字没有消失 —— 它变成了 `title` 与 `aria-label`：
 * 鼠标悬停能看到，屏幕阅读器也能读到（纯图标最容易丢的就是无障碍名称）。
 *
 * **图标必须语义准确**（这条被明确提过）：
 *   · 查看详情 → `Eye`（不是分享/外链图标）；`ExternalLink` 只用于"离开本应用"；
 *   · 编辑 → `PencilLine`；
 *   · 删除 → `Trash2`；
 *   · 禁用/启用 → `Ban` / `CheckCircle2`。
 *
 * 图标块固定 `size-8`（与 `Button size="icon-sm"` 一致），
 * 这样即使某个页面多一个动作，行高也不会跳。
 */
export const RowActions = ({
  actions,
  className,
}: {
  actions: RowAction[];
  className?: string;
}) => {
  if (actions.length === 0) {
    return null;
  }

  return (
    <div className={cn("flex items-center justify-center gap-0.5", className)}>
      {actions.map((action) => {
        const Icon = action.icon;
        return (
          <button
            key={action.id}
            type="button"
            disabled={action.disabled}
            onClick={(event) => {
              // 行本身可能可点（跳详情）；操作按钮不该连带触发行点击。
              event.stopPropagation();
              action.onClick();
            }}
            aria-label={action.label}
            title={action.label}
            className={cn(
              "inline-flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors",
              "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
              "disabled:cursor-not-allowed disabled:pointer-events-none disabled:opacity-40",
              toneClass[action.tone ?? "default"],
            )}
          >
            <Icon className="size-4" aria-hidden="true" />
          </button>
        );
      })}
    </div>
  );
};
