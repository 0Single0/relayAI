import { Area, AreaChart, ResponsiveContainer } from "recharts";
import { cn } from "@/lib/utils";

export type SparklineProps = {
  /** 数值序列（旧 → 新）。空数组时画一条平线。 */
  values: number[];
  /** 线的颜色，默认主色。 */
  color?: string;
  height?: number;
  className?: string;
  /** 无障碍描述。没有它，屏幕阅读器只会看到一张空图。 */
  label?: string;
};

/**
 * 迷你趋势线（指标卡右侧那种小曲线）。
 *
 * 用 recharts 而不是手写 SVG path：折线缩放、留白、数据不足时的降级
 * 都由图表库处理，手写会在"只有 2 个点"或"全是 0"时画出奇怪的形状。
 *
 * 刻意不画坐标轴、网格、tooltip，并且**整体禁用指针交互**
 * （`pointer-events-none`）：卡片本身是引导点击的区域，里面的装饰曲线
 * 如果能被点中，就会出现"点曲线没反应、点旁边才跳转"的诡异手感；
 * recharts 还会在 hover 时接管光标与 tooltip，让整张卡看起来像图表控件。
 * 曲线只表达"形状"（在涨还是在跌），精确数值由左边的数字给。
 */
export const Sparkline = ({
  values,
  color = "var(--chart-1)",
  height = 40,
  className,
  label,
}: SparklineProps) => {
  // 至少两个点才能连成线；不足时给一条水平线，而不是空白。
  const safe = values.length >= 2 ? values : [0, 0];
  const peak = Math.max(...safe.map((value) => Math.abs(value)), 0);

  // 渐变 id 必须全页唯一：多张卡用同一个 id 时，第一张的填充色会串到其它卡上。
  const gradientId = `spark-${color.replace(/[^a-z0-9]/gi, "")}-${safe.length}`;

  const data = safe.map((value, index) => ({ index, value }));

  return (
    <div
      className={cn("pointer-events-none w-full select-none", className)}
      style={{ height }}
      role="img"
      aria-label={`${label ?? "趋势"}，峰值 ${peak}`}
    >
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 2, right: 0, bottom: 2, left: 0 }}>
          <defs>
            <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity={0.28} />
              <stop offset="100%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>
          <Area
            type="monotone"
            dataKey="value"
            stroke={color}
            strokeWidth={1.5}
            fill={`url(#${gradientId})`}
            dot={false}
            isAnimationActive={false}
            activeDot={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
};
