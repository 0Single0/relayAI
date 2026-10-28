import { Check, ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { cn } from "@/lib/utils";

export type ChannelPickerOption = {
  value: string;
  label: string;
  /** 倍率，显示在名称后面，帮助判断"便宜还是贵"。 */
  hint?: string;
};

export type ChannelPickerProps = {
  value: string;
  options: ChannelPickerOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  ariaLabel?: string;
  placeholder?: string;
};

/**
 * 表格内联的渠道选择器。
 *
 * **外观逐项对齐全局 `Select`**（`components/ui/select.tsx` 的 `SelectTrigger` /
 * `SelectContent` / `SelectItem`），但**不复用**那个组件 —— 这里需要
 * "值必须存在、没有空选项"的语义，而全局 Select 的 placeholder 与空值处理
 * 会引入一个"未选择"态，那是我们要去掉的东西。
 *
 * 从全局 Select 抄过来的具体项：
 * | 项 | 值 |
 * | --- | --- |
 * | 触发器高度 | `h-7`（对应 `data-[size=sm]`） |
 * | 圆角 / 边框 | `rounded-md border border-input` |
 * | 悬停 | `hover:border-border-strong` |
 * | 聚焦 | `focus-visible:ring-3 focus-visible:ring-ring/30` |
 * | 箭头 | `ChevronDown size-4 text-muted-foreground` |
 * | 浮层 | `rounded-md border bg-popover p-1 shadow-2` |
 * | 选项 | `rounded-sm py-1.5 pr-8 pl-2 text-sm`，选中项右侧打勾 |
 *
 * 宽度问题：触发器 `w-full`，**由列宽决定可用空间**，
 * 不再套一层 `max-w` —— 渠道名（如"WorkBuddy 国内版"）需要完整显示，
 * 二次限制宽度只会把尾巴截掉。
 */
export const ChannelPicker = ({
  value,
  options,
  onChange,
  disabled,
  ariaLabel = "选择渠道",
  placeholder = "选择渠道",
}: ChannelPickerProps) => {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const listId = useId();

  useEffect(() => {
    if (!open) {
      return;
    }

    const onPointerDown = (event: MouseEvent | TouchEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
      }
    };

    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("touchstart", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    listRef.current?.querySelector<HTMLButtonElement>("button")?.focus();

    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("touchstart", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const current = options.find((option) => option.value === value);

  return (
    <div ref={wrapRef} className="relative w-full">
      <button
        type="button"
        disabled={disabled}
        onClick={(event) => {
          // 行本身可能可点；别连带触发行点击。
          event.stopPropagation();
          setOpen((prev) => !prev);
        }}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        className={cn(
          // 与 SelectTrigger size=sm 同款
          "flex h-7 w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-input bg-background px-2.5 text-xs whitespace-nowrap transition-colors outline-none select-none",
          "hover:border-border-strong",
          "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30",
          "disabled:cursor-not-allowed disabled:opacity-50",
        )}
      >
        <span className={cn("min-w-0 truncate", !current && "text-muted-foreground")}>
          {current ? current.label : placeholder}
        </span>
        <ChevronDown
          className="size-4 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
      </button>

      {open ? (
        <div
          ref={listRef}
          id={listId}
          role="listbox"
          aria-label={ariaLabel}
          className={cn(
            // 与 SelectContent 同款；宽度跟随触发器，长渠道名不换行
            "absolute top-full left-0 z-30 mt-1 max-h-72 w-full min-w-[12rem] overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-2",
          )}
        >
          {options.map((option) => {
            const active = option.value === value;
            return (
              <button
                key={option.value}
                type="button"
                role="option"
                aria-selected={active}
                onClick={(event) => {
                  event.stopPropagation();
                  setOpen(false);
                  // 选中同一项不触发写库。
                  if (!active) {
                    onChange(option.value);
                  }
                }}
                className={cn(
                  // 与 SelectItem 同款
                  "relative flex w-full cursor-pointer items-center gap-2 rounded-sm py-1.5 pr-8 pl-2 text-left text-xs outline-none select-none",
                  "focus:bg-accent focus:text-accent-foreground",
                  active ? "bg-accent/60" : "hover:bg-muted",
                )}
              >
                <span className="min-w-0 flex-1 truncate">{option.label}</span>
                {option.hint ? (
                  <span className="shrink-0 text-[11px] text-muted-foreground">
                    {option.hint}
                  </span>
                ) : null}
                {active ? (
                  <span className="absolute right-2 flex size-3.5 items-center justify-center">
                    <Check className="size-4" aria-hidden="true" />
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
      ) : null}
    </div>
  );
};
