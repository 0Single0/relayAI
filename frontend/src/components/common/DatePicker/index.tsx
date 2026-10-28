import { Check, ChevronLeft, ChevronRight, X } from "lucide-react";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export type DateRange = {
  /** ISO 日期，格式 YYYY-MM-DD；空串表示不限。 */
  from: string;
  to: string;
};

export type DatePickerProps = {
  value: DateRange;
  onChange: (next: DateRange) => void;
  label?: string;
  disabled?: boolean;
  className?: string;
};

const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"];

const pad = (value: number) => String(value).padStart(2, "0");

/** 把 Date 转成本地 YYYY-MM-DD（不用 toISOString，避免时区偏移）。 */
const toKey = (date: Date) =>
  `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;

const parseKey = (key: string): Date | null => {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(key)) {
    return null;
  }

  const [year, month, day] = key.split("-").map(Number);
  const date = new Date(year, month - 1, day);

  return Number.isNaN(date.getTime()) ? null : date;
};

/** 该月第一天是周几（周一 = 0）。 */
const firstWeekday = (year: number, month: number) =>
  (new Date(year, month, 1).getDay() + 6) % 7;

const daysInMonth = (year: number, month: number) =>
  new Date(year, month + 1, 0).getDate();

/**
 * 日期范围选择器。
 *
 * 不用原生 `input[type=date]`：AGENTS.md 明确禁止，且原生的样式与主题脱节、
 * 键盘行为在各浏览器不一致。这里自己实现网格 + 键盘导航。
 */
export const DatePicker = ({
  value,
  onChange,
  label = "时间范围",
  disabled = false,
  className,
}: DatePickerProps) => {
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(() => {
    const base = parseKey(value.from) ?? new Date();
    return { year: base.getFullYear(), month: base.getMonth() };
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();

  // 点外部关闭 + Esc 关闭。
  useEffect(() => {
    if (!open) {
      return;
    }

    const onPointerDown = (event: MouseEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    };

    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);

    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const grid = useMemo(() => {
    const offset = firstWeekday(cursor.year, cursor.month);
    const total = daysInMonth(cursor.year, cursor.month);
    const cells: Array<string | null> = Array.from({ length: offset }, () => null);

    for (let day = 1; day <= total; day += 1) {
      cells.push(`${cursor.year}-${pad(cursor.month + 1)}-${pad(day)}`);
    }

    return cells;
  }, [cursor]);

  const pick = useCallback(
    (key: string) => {
      const { from, to } = value;

      // 第一次点选起点；第二次点选终点（若早于起点则交换）。
      if (!from || (from && to)) {
        onChange({ from: key, to: "" });
        return;
      }

      if (key < from) {
        onChange({ from: key, to: from });
      } else {
        onChange({ from, to: key });
      }
    },
    [onChange, value],
  );

  const shiftMonth = useCallback((delta: number) => {
    setCursor((prev) => {
      const next = new Date(prev.year, prev.month + delta, 1);
      return { year: next.getFullYear(), month: next.getMonth() };
    });
  }, []);

  const summary = useMemo(() => {
    if (!value.from && !value.to) {
      return "全部时间";
    }
    if (value.from && !value.to) {
      return `${value.from} 起`;
    }
    return `${value.from} → ${value.to}`;
  }, [value]);

  // 今天的高亮键：用 state 存一次，避免渲染期调用 impure 的 new Date()
  // （React Compiler 会因此跳过整个组件的优化）。
  const [todayKey, setTodayKey] = useState(() => toKey(new Date()));

  useEffect(() => {
    setTodayKey(toKey(new Date()));
  }, []);

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      <button
        ref={triggerRef}
        type="button"
        disabled={disabled}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen((prev) => !prev)}
        className={cn(
          "flex h-8 w-full items-center justify-between gap-2 rounded-md border border-input bg-background px-2.5 text-sm transition-colors",
          "hover:border-border-strong",
          "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
          "disabled:cursor-not-allowed disabled:opacity-50",
        )}
      >
        <span className="truncate">{summary}</span>
        <span className="shrink-0 text-xs text-muted-foreground">{label}</span>
      </button>

      {open ? (
        <div
          id={panelId}
          role="dialog"
          aria-label={`${label}选择`}
          className="absolute top-9 left-0 z-50 w-72 rounded-lg border bg-popover p-3 shadow-3"
        >
          <div className="flex items-center justify-between gap-2">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="上一月"
              onClick={() => shiftMonth(-1)}
            >
              <ChevronLeft aria-hidden="true" />
            </Button>
            <span className="text-sm font-medium">
              {cursor.year} 年 {cursor.month + 1} 月
            </span>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="下一月"
              onClick={() => shiftMonth(1)}
            >
              <ChevronRight aria-hidden="true" />
            </Button>
          </div>

          <div className="mt-2 grid grid-cols-7 gap-0.5 text-center text-[11px] text-muted-foreground">
            {WEEKDAYS.map((day) => (
              <span key={day} className="py-1">
                {day}
              </span>
            ))}
          </div>

          <div className="grid grid-cols-7 gap-0.5">
            {grid.map((key, index) =>
              key === null ? (
                <span key={`blank-${index}`} />
              ) : (
                <button
                  key={key}
                  type="button"
                  onClick={() => pick(key)}
                  aria-pressed={
                    key === value.from || key === value.to ? true : undefined
                  }
                  className={cn(
                    "flex h-8 items-center justify-center rounded-sm text-xs transition-colors",
                    "hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                    key === value.from || key === value.to
                      ? "bg-primary text-primary-foreground hover:bg-primary"
                      : key > (value.from || "") && key < (value.to || "\uffff")
                        ? "bg-accent text-accent-foreground"
                        : "",
                    key === todayKey && key !== value.from && key !== value.to
                      ? "font-semibold text-primary"
                      : "",
                  )}
                >
                  {Number(key.slice(-2))}
                </button>
              ),
            )}
          </div>

          <div className="mt-2 flex items-center justify-between gap-2 border-t pt-2">
            <Button
              variant="ghost"
              size="xs"
              onClick={() => onChange({ from: "", to: "" })}
            >
              <X aria-hidden="true" />
              清除
            </Button>
            <Button
              variant="outline"
              size="xs"
              onClick={() => {
                const key = toKey(new Date());
                onChange({ from: key, to: key });
              }}
            >
              <Check aria-hidden="true" />
              今天
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
};
