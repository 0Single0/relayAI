import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * 图表 tooltip 的数据形状。
 *
 * 不从 recharts 的 `TooltipProps` 取类型：那套类型把 payload 定义成
 * 一堆可选字段，写起来到处要判空，而这里实际只需要「名字 / 值 / 颜色」三样。
 * 自定义 tooltip 的价值就在于不复用它的默认渲染，因此也不必复用它的类型。
 */
export type TooltipEntry = {
  name?: string | number;
  /**
   * recharts 的 `ValueType` 允许数组（多值柱），这里用 `unknown` 接住，
   * 由 `toNumber` 负责收敛成一个数。写死成 number 会让整个
   * `content` 回调的签名与 recharts 的 props 不兼容。
   */
  value?: unknown;
  color?: string;
  /**
   * recharts 允许把 `dataKey` 写成取值函数（`(obj) => any`），
   * 所以这里必须容纳它，否则整个 `content` 回调无法赋给 `Tooltip` 的
   * `content` 属性。实际用时我们只按字符串匹配序列。
   */
  dataKey?: unknown;
  payload?: Record<string, unknown>;
};

export type TooltipPayload = {
  active?: boolean;
  label?: string | number;
  /**
   * recharts 传进来的 payload 是 `readonly` 数组，这里必须跟着只读，
   * 否则整个 `content` 回调的函数签名对不上 `TooltipContentProps`
   * （TS 会报 "readonly cannot be assigned to mutable"）。
   */
  payload?: readonly TooltipEntry[];
};

export type ChartTooltipProps = TooltipPayload & {
  /** 数值格式化。金额、token、百分比各页不同，由调用方决定。 */
  formatValue?: (value: number, entry: TooltipEntry) => string;
  /** 底部补充行（例如该时间点的成功率）。 */
  footer?: (first: TooltipEntry) => ReactNode;
  /** 单位后缀，例如 "token"。 */
  unit?: string;
};

const toNumber = (value: unknown) => {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
};

const defaultFormat = (value: number) => value.toLocaleString("zh-CN");

/**
 * 自定义 tooltip。
 *
 * 与 recharts 默认样式的区别（这些都是刻意的）：
 *   · 默认浮层是"白底 + 1px 灰边"，与卡片几乎同色，压不住图表；
 *     这里用 popover 底 + 中等阴影 + 加粗标题，能明确浮在图表之上。
 *   · 默认把所有序列平铺成一行行灰字；这里每条序列前面加同色圆点，
 *     颜色与折线/柱子一致 —— 多序列时不用回忆"哪条是哪个颜色"。
 *   · 默认标题是横轴原值；这里单独一行、加底部分隔线，把
 *     "什么时间点" 与 "各项分别是多少" 分开读。
 *   · 默认没有单位；这里把单位统一放在标题右侧，避免每行都重复一遍。
 *
 * 数值一律 `tabular-nums`：浮层会跟着鼠标移动，等宽数字不会让浮层抖动。
 */
export const ChartTooltip = ({
  active,
  label,
  payload,
  formatValue = defaultFormat,
  footer,
  unit,
}: ChartTooltipProps) => {
  if (!active || !payload || payload.length === 0) {
    return null;
  }

  // 值为 0 的序列仍然显示：用户要找的是"这一小时为什么没量"，
  // 隐藏 0 会让浮层只列出有值的项，反而看不清全貌。
  const rows = payload.filter((entry) => entry.name !== undefined);

  return (
    <div
      className={cn(
        "pointer-events-none min-w-[9rem] max-w-[16rem] rounded-lg border",
        "bg-popover px-3 py-2 text-xs text-popover-foreground shadow-[var(--shadow-3)]",
      )}
      role="status"
    >
      <div className="flex items-baseline justify-between gap-3 border-b border-border/70 pb-1.5">
        <span className="font-medium">{label}</span>
        {unit ? (
          <span className="shrink-0 text-[10px] text-muted-foreground">{unit}</span>
        ) : null}
      </div>

      <ul className="mt-1.5 flex flex-col gap-1">
        {rows.map((entry, index) => (
          <li
            key={`${String(entry.dataKey ?? entry.name)}-${index}`}
            className="flex items-center gap-2"
          >
            <span
              className="size-1.5 shrink-0 rounded-full"
              style={{ backgroundColor: entry.color ?? "var(--chart-1)" }}
              aria-hidden="true"
            />
            <span className="min-w-0 flex-1 truncate text-muted-foreground">
              {entry.name}
            </span>
            <span className="shrink-0 font-medium tabular-nums">
              {formatValue(toNumber(entry.value), entry)}
            </span>
          </li>
        ))}
      </ul>

      {footer && rows.length > 0 ? (
        <div className="mt-1.5 border-t border-border/70 pt-1.5 text-[11px] text-muted-foreground">
          {footer(rows[0])}
        </div>
      ) : null}
    </div>
  );
};
