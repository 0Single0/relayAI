import type { LucideIcon } from "lucide-react";
import { Inbox } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export type EmptyStateProps = {
  /** 图标，默认用收件箱。 */
  icon?: LucideIcon;
  title: string;
  description?: string;
  /** 下一步动作（按钮或链接）。空态必须给出"我该做什么"。 */
  action?: ReactNode;
  className?: string;
};

/**
 * 统一的空态。
 *
 * 规范要求空态写"一句话说明 + 下一步动作"，所以 `description` 与 `action`
 * 在实际使用中应当至少出现一个，不要只放一个图标。
 */
export const EmptyState = ({
  icon: Icon = Inbox,
  title,
  description,
  action,
  className,
}: EmptyStateProps) => (
  <div
    className={cn(
      "flex flex-col items-center justify-center gap-2 px-4 py-10 text-center",
      className,
    )}
  >
    <div className="flex size-10 items-center justify-center rounded-full border bg-muted/40">
      <Icon className="size-4 text-muted-foreground" aria-hidden="true" />
    </div>
    <p className="text-sm font-medium">{title}</p>
    {description ? (
      <p className="max-w-sm text-xs leading-5 text-muted-foreground">
        {description}
      </p>
    ) : null}
    {action ? <div className="mt-2">{action}</div> : null}
  </div>
);
