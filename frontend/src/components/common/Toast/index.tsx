import { useEffect, useState } from "react";
import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import {
  dismiss,
  snapshot,
  subscribe,
  type ToastItem,
  type ToastTone,
} from "@/components/common/Toast/store";
import { cn } from "@/lib/utils";

const TONE_STYLE: Record<ToastTone, { icon: typeof Info; className: string }> = {
  success: { icon: CircleCheck, className: "text-success" },
  error: { icon: CircleAlert, className: "text-destructive" },
  info: { icon: Info, className: "text-primary" },
};

/**
 * 每种 tone 的容器配色。
 *
 * 成功用**实心绿底 + 白字**：它通常是"操作完成"的确认，
 * 需要一眼看出结果，不该和普通提示长得一样。
 * 失败与信息用浅色底 + 深色字：它们常伴随多行原因，
 * 实心底会把整块压得很重。
 */
const TONE_SURFACE: Record<ToastTone, string> = {
  success: "border-success bg-success text-success-foreground",
  error: "border-destructive/40 bg-destructive/5",
  info: "border-border bg-popover",
};

/** 副文案在实心底上要用半透明白，否则对比度不足。 */
const TONE_DETAIL: Record<ToastTone, string> = {
  success: "text-success-foreground/85",
  error: "text-muted-foreground",
  info: "text-muted-foreground",
};

/**
 * Toast 视口：挂在应用根部一次即可。
 *
 * 位置选右下角：
 *   · 右上角与页头的用户菜单冲突；
 *   · 底部居中会盖住表格分页条。
 *
 * 移动端占满宽度（`calc(100vw-2rem)`），否则窄屏下贴着右边会挤压正文。
 */
export const ToastViewport = () => {
  const [items, setItems] = useState<ToastItem[]>(snapshot);

  useEffect(() => subscribe(setItems), []);

  if (items.length === 0) {
    return null;
  }

  return (
    <div
      role="region"
      aria-label="通知"
      /*
        顶部居中。

        为什么不是右下角：右下角与表格分页条、以及页面底部的操作区重叠，
        而右上角被用户菜单占着。顶部居中是视线起点，
        操作完抬眼看得到，也不挡任何控件。

        `top-16` 让开 56px 的页头（h-14），否则会压在标题上。
      */
      className="pointer-events-none fixed top-16 left-1/2 z-50 flex w-[min(24rem,calc(100vw-2rem))] -translate-x-1/2 flex-col gap-2"
    >
      {items.map((item) => {
        const { icon: Icon } = TONE_STYLE[item.tone];
        const solid = item.tone === "success";
        return (
          <div
            key={item.id}
            role="status"
            aria-live={item.tone === "error" ? "assertive" : "polite"}
            className={cn(
              "pointer-events-auto flex items-start gap-2 rounded-lg border p-3 shadow-lg",
              "toast-rise",
              TONE_SURFACE[item.tone],
            )}
          >
            <Icon
              className={cn(
                "mt-0.5 size-4 shrink-0",
                // 实心底上图标跟正文同色，否则绿底上的绿图标看不见。
                solid ? "text-success-foreground" : TONE_STYLE[item.tone].className,
              )}
              aria-hidden="true"
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium">{item.title}</p>
              {item.description ? (
                <p className={cn("mt-0.5 break-words text-xs leading-5", TONE_DETAIL[item.tone])}>
                  {item.description}
                </p>
              ) : null}
            </div>
            <button
              type="button"
              onClick={() => dismiss(item.id)}
              aria-label="关闭通知"
              className={cn(
                "cursor-pointer rounded p-0.5 transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                solid
                  ? "text-success-foreground/70 hover:bg-success-foreground/15 hover:text-success-foreground"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
            >
              <X className="size-3.5" aria-hidden="true" />
            </button>
          </div>
        );
      })}
    </div>
  );
};
