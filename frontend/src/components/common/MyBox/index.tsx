import { LoaderCircle } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export type MyBoxProps = {
  isLoading?: boolean;
  text?: string;
  spinnerSize?: number;
  className?: string;
  children?: ReactNode;
};

export const MyBox = ({
  isLoading = false,
  text = "加载中...",
  spinnerSize = 20,
  className,
  children,
}: MyBoxProps) => (
  <div
    /*
      外层默认 `flex flex-col`：调用方普遍传 `flex-1 flex-col` 想让它撑满剩余高度，
      而里面那层用的是 `h-full` —— 如果外层不是 flex 容器，`h-full` 拿不到
      确定的高度，于是整块缩成内容高度（表格就变成 fit-content）。
      在这里兜住：外层是 flex 列容器，内层 `h-full` 才有意义。
    */
    className={cn("relative flex min-h-16 flex-col", className)}
    aria-busy={isLoading || undefined}
  >
    <div
      className={cn(
        // 必须是 `flex flex-col`：调用方（表格）传的是 `min-h-0 flex-1 overflow-auto`，
        // 那些类只有在 flex 父容器里才有意义。父层不是 flex 时 `flex-1` 无效，
        // 表格会按内容高度溢出，压到下方的分页条上（实测重叠）。
        //
        // 用 `flex-1` 而不是 `h-full`：外层同时有 `min-h-16`，
        // `h-full` 会按"父容器高度"算，而父容器高度由 `min-h-16` 托底时
        // 算出来的值可能大于实际可用空间，表格就会顶出去、盖住分页条。
        // `flex-1 min-h-0` 是"占满剩余空间"的标准写法，不依赖父级高度已确定。
        "flex min-h-0 flex-1 flex-col transition-opacity duration-200",
        isLoading && "pointer-events-none opacity-30 select-none",
      )}
      aria-hidden={isLoading || undefined}
    >
      {children}
    </div>

    {isLoading ? (
      <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 rounded-[inherit] bg-background/50 px-4 text-center">
        <LoaderCircle
          className="animate-spin text-muted-foreground motion-reduce:animate-none"
          style={{ width: spinnerSize, height: spinnerSize }}
          aria-hidden="true"
        />
        {/*
          `text` 为空时**不渲染那一行**：紧凑区块（一排状态卡）只需要一个转圈，
          带上"加载中..."反而把它撑高、把下方内容挤动。
        */}
        {text ? (
          <span className="text-xs text-muted-foreground" role="status">
            {text}
          </span>
        ) : null}
      </div>
    ) : null}
  </div>
);
