import { type ReactNode, useCallback, useEffect, useState } from "react";
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Legend,
  Pie,
  PieChart,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { ChartTooltip, type TooltipPayload } from "@/components/common/Charts/ChartTooltip";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";

/**
 * 量一次容器的内容盒尺寸。
 *
 * **为什么不用 recharts 的 `ResponsiveContainer`**：它内部先渲染一个 0×0 的占位，
 * 再靠 `ResizeObserver` 补量。本应用的外壳是"只有内容区内部滚动"（`main` 带
 * `overflow-y`），首帧量到的宽度是 0，之后 ResizeObserver 的回调在滚动容器里
 * 不一定重触发，于是 SVG 定型成 7×7 的"图钉"——图渲染了但看不见。
 * 实测就是这么塌的（`svg` 的 `width/height` 都是 `7`）。
 *
 * 自己量就没有这个回路：先同步读一次 `getBoundingClientRect`，再补一帧
 * `requestAnimationFrame`，最后才挂 `ResizeObserver`。宽高以**确定数值**传给
 * 图表，尺寸只跟容器走，与滚动无关。
 */
const useChartSize = () => {
  const [node, setNode] = useState<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

  // callback ref：图表是数据到了才挂载的，object ref 不会触发重测。
  const ref = useCallback((element: HTMLDivElement | null) => setNode(element), []);

  useEffect(() => {
    if (!node) {
      return;
    }

    const read = () => {
      const rect = node.getBoundingClientRect();
      const style = getComputedStyle(node);
      const width = Math.max(
        0,
        Math.round(
          rect.width -
            parseFloat(style.borderLeftWidth || "0") -
            parseFloat(style.borderRightWidth || "0"),
        ),
      );
      const height = Math.max(
        0,
        Math.round(
          rect.height -
            parseFloat(style.borderTopWidth || "0") -
            parseFloat(style.borderBottomWidth || "0"),
        ),
      );
      setSize((current) =>
        current.width === width && current.height === height
          ? current
          : { width, height },
      );
    };

    // 先同步量一次：ResizeObserver 的初始回调时机不保证（后台标签会被节流）。
    read();
    const frame = requestAnimationFrame(read);

    if (typeof ResizeObserver === "undefined") {
      return () => cancelAnimationFrame(frame);
    }
    const observer = new ResizeObserver(read);
    observer.observe(node);

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [node]);

  return [ref, size] as const;
};

/**
 * 图表公共样式。
 *
 * 坐标轴与网格的样式集中在这里，是因为每张图各写一遍会立刻漂移：
 * 一个页面里出现三种字号、两种虚线，看起来就像两拨人做的。
 */
const AXIS_TICK = { fontSize: 11, fill: "var(--muted-foreground)" } as const;
const AXIS_STROKE = "var(--border)" as const;
const GRID_STROKE = "var(--border)" as const;

const axisProps = {
  tick: AXIS_TICK,
  stroke: AXIS_STROKE,
  tickLine: false,
  axisLine: false,
} as const;

/** 图例文字尺寸统一。 */
const legendStyle = { fontSize: 11, color: "var(--muted-foreground)" } as const;

/**
 * tooltip 的鼠标指示线：用一条淡色竖线而不是默认的方块底色。
 *
 * 方块底色会把柱子整个罩住，看不见柱子本身；竖线只标出"这一列"，
 * 既有定位感又不遮挡数据。
 */
const cursorLine = { stroke: "var(--border-strong)", strokeDasharray: "3 3" } as const;
const cursorColumn = { fill: "var(--muted)", fillOpacity: 0.6 } as const;

export type TrendSeries = {
  key: string;
  label: string;
  color: string;
  /** 这一条序列的单位（浮层里显示）。 */
  unit?: string;
  /** 这一条序列的数值格式化。 */
  formatValue?: (value: number) => string;
};

export type TrendChartProps = {
  data: Array<Record<string, unknown>>;
  /** 横轴字段名（已格式化好的标签）。 */
  xKey: string;
  /** 要画的数值字段，可多条。 */
  series: TrendSeries[];
  height?: number;
  className?: string;
  /** 是否用面积图；要柱状时显式传 `false`（统计报表页）。 */
  area?: boolean;
  /**
   * 多条面积序列是否堆叠。
   *
   * 堆叠解决"互相遮挡"：输入/输出 Token 相加就是总量，堆叠后的总高度
   * 有实际含义。不堆叠的多层半透明面积会变成第三种颜色，读不出来。
   */
  stacked?: boolean;
  /** 纵轴是否用 K/M 缩写。 */
  compactAxis?: boolean;
  /** 是否显示图例（默认多序列显示）。 */
  showLegend?: boolean;
  /** 浮层底部补充行（例如该时间点的成功率）。 */
  tooltipFooter?: (row: Record<string, unknown>) => ReactNode;
};

/**
 * 趋势图（面积 / 柱状）。
 *
 * 默认面积图：趋势要的是"形状"，面积填充比柱子更容易看出起伏。
 * 多条序列时也是面积图（各条独立半透明填充 + 实线描边），这与参考实现一致：
 * 柱状图在"输入/输出/总量"这种量级差很大的组合里，小的那条会被压成看不见的细线。
 * `stacked` 给"各序列相加等于总量"的场景用（堆叠高度即总量）。
 * 需要柱状时显式传 `area={false}`（统计报表页就是）。
 */
export const TrendChart = ({
  data,
  xKey,
  series,
  height = 220,
  className,
  area = true,
  stacked = false,
  compactAxis = true,
  showLegend,
  tooltipFooter,
}: TrendChartProps) => {
  const useArea = area;
  const withLegend = showLegend ?? series.length > 1;

  /**
   * 面积填充的顶部不透明度。
   *
   * 多条曲线共用一个坐标系时，每条都是一个"从线到 0"的填充块，
   * 后画的会盖住先画的。填充太重时只能看见最上面那条，其余全被糊掉；
   * 所以条数越多，填充越淡 —— 让四条描边都清晰可读，填充只用来区分区域。
   */
  const fillTop = series.length > 2 ? 0.16 : series.length === 2 ? 0.22 : 0.28;

  // 多序列时每条单位可能不同（如"请求数"与"token"），所以不设统一 unit，
  // 让格式化函数自己带单位，避免浮层标题旁边的单位误导。
  const unit = series.length === 1 ? series[0].unit : undefined;

  const renderTooltip = (props: TooltipPayload) => {
    const first = props.payload?.[0]?.payload;
    const single = series.length === 1 ? series[0] : null;

    return (
      <ChartTooltip
        active={props.active}
        label={props.label}
        payload={props.payload}
        unit={unit}
        formatValue={(value, entry) => {
          // 按 dataKey 找回这条序列自己的格式化器：多序列时每条口径不同。
          const match = series.find((item) => item.key === entry.dataKey);
          const formatter = match?.formatValue ?? single?.formatValue;
          return formatter ? formatter(value) : value.toLocaleString("zh-CN");
        }}
        footer={tooltipFooter && first ? () => tooltipFooter(first) : undefined}
      />
    );
  };

  const [containerRef, { width, height: boxHeight }] = useChartSize();
  // 传了 height 就用它当高度上限；没传就跟着容器走。
  const resolvedHeight = Math.max(0, Math.min(boxHeight || height, height));

  return (
    <div
      ref={containerRef}
      className={cn("w-full", className)}
      style={{ height: resolvedHeight || height }}
    >
      {width > 0 ? (
        <AreaChartOrBar
          width={width}
          height={resolvedHeight}
          data={data}
          xKey={xKey}
          series={series}
          useArea={useArea}
          stacked={stacked}
          compactAxis={compactAxis}
          withLegend={withLegend}
          fillTop={fillTop}
          renderTooltip={renderTooltip}
        />
      ) : null}
    </div>
  );
}

/**
 * 面积图与柱状图的实际渲染体。
 *
 * 单独抽出来是为了让 `TrendChart` 里那段"要么面积要么柱状"的分支
 * 不把两个完整图表定义挤在一个 return 里（超过三层嵌套就很难读）。
 */
type ChartBodyProps = {
  width: number;
  height: number;
  data: Array<Record<string, unknown>>;
  xKey: string;
  series: TrendSeries[];
  useArea: boolean;
  stacked: boolean;
  compactAxis: boolean;
  withLegend: boolean;
  fillTop: number;
  renderTooltip: (props: TooltipPayload) => ReactNode;
};

const AreaChartOrBar = ({
  width,
  height,
  data,
  xKey,
  series,
  useArea,
  stacked,
  compactAxis,
  withLegend,
  fillTop,
  renderTooltip,
}: ChartBodyProps) => {
  if (height <= 0) {
    return null;
  }

  if (!useArea) {
    return (
      <BarChart
        width={width}
        height={height}
        data={data}
        margin={{ top: 8, right: 8, bottom: 0, left: -18 }}
      >
        <CartesianGrid stroke={GRID_STROKE} strokeDasharray="3 3" vertical={false} />
        <XAxis dataKey={xKey} interval="preserveStartEnd" {...axisProps} />
        <YAxis
          tickFormatter={compactAxis ? compactNumber : undefined}
          width={48}
          {...axisProps}
        />
        <Tooltip content={renderTooltip} cursor={cursorColumn} />
        {withLegend ? (
          <Legend wrapperStyle={legendStyle} iconType="circle" iconSize={7} />
        ) : null}
        {series.map((item) => (
          <Bar
            key={item.key}
            dataKey={item.key}
            name={item.label}
            fill={item.color}
            radius={[3, 3, 0, 0]}
            maxBarSize={28}
            isAnimationActive={false}
          />
        ))}
      </BarChart>
    );
  }

  return (
    <AreaChart
      width={width}
      height={height}
      data={data}
      margin={{ top: 8, right: 8, bottom: 0, left: -18 }}
    >
      <defs>
        {series.map((item) => (
          <linearGradient
            key={item.key}
            id={`fill-${item.key}`}
            x1="0"
            y1="0"
            x2="0"
            y2="1"
          >
            <stop offset="0%" stopColor={item.color} stopOpacity={fillTop} />
            <stop offset="100%" stopColor={item.color} stopOpacity={0.01} />
          </linearGradient>
        ))}
      </defs>
      <CartesianGrid stroke={GRID_STROKE} strokeDasharray="3 3" vertical={false} />
      <XAxis dataKey={xKey} interval="preserveStartEnd" {...axisProps} />
      <YAxis
        tickFormatter={compactAxis ? compactNumber : undefined}
        width={48}
        {...axisProps}
      />
      <Tooltip content={renderTooltip} cursor={cursorLine} />
      {withLegend ? (
        <Legend wrapperStyle={legendStyle} iconType="circle" iconSize={7} />
      ) : null}
      {series.map((item, index) => (
        <Area
          key={item.key}
          type="monotone"
          dataKey={item.key}
          name={item.label}
          // 堆叠顺序 = 数组顺序：第一条在最底下。
          stackId={stacked ? "stack" : undefined}
          stroke={item.color}
          strokeWidth={1.75}
          fill={`url(#fill-${item.key})`}
          fillOpacity={stacked ? 0.6 : 1}
          dot={false}
          activeDot={index === series.length - 1 ? { r: 3, strokeWidth: 0 } : false}
          /*
            关掉入场动画。
            两个原因：
            1. 仪表盘会定时刷新，动画开着的话每次拿到新数据整张图都从零重画，
               看起来像在闪；
            2. 动画期间 recharts 是把 path 的 `d` 从基线逐渐插值过去的。
               截图或首屏恰好落在起始帧时，`d` 已经存在、但只画出了起点附近
               一小段 —— 表现为"有坐标轴没有曲线"。这种不确定性对监控页面
               没有价值，直接关掉。
          */
          isAnimationActive={false}
        />
      ))}
    </AreaChart>
  );
};

export type DonutDatum = { name: string; value: number; color: string };

export type DonutChartProps = {
  data: DonutDatum[];
  height?: number;
  /** 圆心中央的主文案（通常是总量）。 */
  centerLabel?: string;
  centerValue?: string;
  /** 浮层里的单位。 */
  unit?: string;
  className?: string;
};

/**
 * 环形图。
 *
 * 用环形而不是饼图：中间那圈空白可以放总量，省掉一个独立的图例块，
 * 在仪表盘这种格子密集的页面上更快读到"构成 + 总量"两件事。
 */
export const DonutChart = ({
  data,
  height = 200,
  centerLabel,
  centerValue,
  unit,
  className,
}: DonutChartProps) => {
  const total = data.reduce((sum, item) => sum + item.value, 0);
  const [containerRef, { width, height: boxHeight }] = useChartSize();
  const resolvedHeight = Math.max(0, Math.min(boxHeight || height, height));

  return (
    <div
      ref={containerRef}
      className={cn("relative w-full", className)}
      style={{ height: resolvedHeight || height }}
    >
      {width > 0 && resolvedHeight > 0 ? (
        <PieChart width={width} height={resolvedHeight}>
          <Tooltip
            content={
              <ChartTooltip
                unit={unit}
                formatValue={(value) =>
                  `${value.toLocaleString("zh-CN")}（${
                    total > 0 ? ((value / total) * 100).toFixed(1) : "0.0"
                  }%）`
                }
              />
            }
          />
          <Pie
            data={data}
            dataKey="value"
            nameKey="name"
            innerRadius="62%"
            outerRadius="88%"
            paddingAngle={2}
            strokeWidth={0}
          >
            {data.map((item) => (
              <Cell key={item.name} fill={item.color} />
            ))}
          </Pie>
        </PieChart>
      ) : null}

      {centerValue ? (
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-lg font-semibold tabular-nums">{centerValue}</span>
          {centerLabel ? (
            <span className="text-[11px] text-muted-foreground">{centerLabel}</span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
};

/** 环形图下方的构成列表（图例太挤，改成带数值的清单更好读）。 */
export const DonutLegend = ({
  data,
  total,
}: {
  data: DonutDatum[];
  total: number;
}) => (
  <ul className="flex flex-col gap-1.5">
    {data.map((item) => (
      <li key={item.name} className="flex items-center gap-2 text-xs">
        <span
          className="size-2 shrink-0 rounded-full"
          style={{ backgroundColor: item.color }}
          aria-hidden="true"
        />
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{item.name}</span>
        <span className="shrink-0 tabular-nums">{item.value.toLocaleString("zh-CN")}</span>
        <span className="w-11 shrink-0 text-right tabular-nums text-muted-foreground">
          {total > 0 ? `${((item.value / total) * 100).toFixed(0)}%` : "0%"}
        </span>
      </li>
    ))}
  </ul>
);

/** 图表空态占位：保持与其他图相同高度，避免加载完成后页面跳动。 */
export const ChartPlaceholder = ({
  height = 220,
  text = "暂无数据",
}: {
  height?: number;
  text?: string;
  children?: ReactNode;
}) => (
  <div
    className="flex items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground"
    style={{ height }}
  >
    {text}
  </div>
);
