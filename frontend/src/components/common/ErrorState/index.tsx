import { CircleAlert, RefreshCw } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export type ErrorStateProps = {
  title?: string;
  message: string;
  onRetry?: () => void;
  retrying?: boolean;
  /** 补充说明，例如"确认后端已启动"。 */
  hint?: ReactNode;
  className?: string;
};

/** 统一的错误态：说明 + 可重试。 */
export const ErrorState = ({
  title = "加载失败",
  message,
  onRetry,
  retrying = false,
  hint,
  className,
}: ErrorStateProps) => (
  <div
    className={cn(
      "flex flex-col items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3",
      className,
    )}
    role="alert"
  >
    <div className="flex items-center gap-2 text-sm font-medium text-destructive">
      <CircleAlert className="size-4" aria-hidden="true" />
      {title}
    </div>
    <p className="text-xs leading-5 break-words text-destructive">{message}</p>
    {hint ? (
      <p className="text-[11px] leading-5 text-muted-foreground">{hint}</p>
    ) : null}
    {onRetry ? (
      <Button variant="outline" size="sm" onClick={onRetry} disabled={retrying}>
        <RefreshCw
          className={cn(retrying && "animate-spin motion-reduce:animate-none")}
          aria-hidden="true"
        />
        {retrying ? "重试中..." : "重试"}
      </Button>
    ) : null}
  </div>
);
