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
  success: { icon: CircleCheck, className: "text-success-foreground" },
  error: { icon: CircleAlert, className: "text-destructive-foreground" },
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
  /*
    失败必须是**实心**底。

    早先写的是 `bg-destructive/5`（5% 不透明度）：
    结果 toast 几乎透明，背后的表格透过来看得一清二楚，
    长错误信息完全读不了。错误是最需要看清的一种提示，
    不能用"淡淡的红"这种装饰性做法。
  */
  error: "border-destructive bg-destructive text-destructive-foreground",
  info: "border-border bg-popover",
};

/** 副文案在实心底上要用半透明白，否则对比度不足。 */
const TONE_DETAIL: Record<ToastTone, string> = {
  success: "text-success-foreground/85",
  error: "text-destructive-foreground/85",
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

        `top-11` = 44px：浮在 56px 页头上方，即遮住页头下部、保留顶部。
        再高（比页头还靠上）会盖到浏览器边缘；再低则重新挤回内容区。
      */
      className="pointer-events-none fixed top-11 left-1/2 z-50 flex w-[min(24rem,calc(100vw-2rem))] -translate-x-1/2 flex-col gap-2"
    >
      {items.map((item) => {
        const { icon: Icon } = TONE_STYLE[item.tone];
        // 成功与失败都用实心底：两者都是"结果"，需要一眼看清。
        return (
          <div
            key={item.id}
            role="status"
            aria-live={item.tone === "error" ? "assertive" : "polite"}
            className={cn(
              /*
                `px-4 py-3.5`：比默认 p-3 更松。

                内边距决定 toast 的"高度感"。p-3 时两行字贴得很紧，
                看起来像薄薄一条横幅；加高后标题与说明才分得开，
                读起来不费力。
              */
              "pointer-events-auto flex items-start gap-2.5 rounded-lg border px-4 py-3.5 shadow-lg",
              "toast-rise",
              TONE_SURFACE[item.tone],
            )}
          >
            <Icon
              className={cn(
                "mt-0.5 size-4 shrink-0",
                // 实心底上图标与正文同色；TONE_STYLE 已给的是 *-foreground。
                TONE_STYLE[item.tone].className,
              )}
              aria-hidden="true"
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm leading-5 font-medium">{item.title}</p>
              {item.description ? (
                <p className={cn("mt-1 break-words text-xs leading-5", TONE_DETAIL[item.tone])}>
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
                // info 是浅底；success / error 是实心底，用前景色的半透明。
                item.tone === "info"
                  ? "text-muted-foreground hover:bg-muted hover:text-foreground"
                  : "text-current opacity-70 hover:bg-current/15 hover:opacity-100",
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
