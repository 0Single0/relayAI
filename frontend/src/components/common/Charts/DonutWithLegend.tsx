import { Cell, Pie, PieChart, Tooltip } from "recharts";
import { ChartTooltip } from "@/components/common/Charts/ChartTooltip";
import { cn } from "@/lib/utils";

export type DonutDatum = { name: string; value: number; color: string };

export type DonutWithLegendProps = {
  data: DonutDatum[];
  /** 图表下方的说明或圆心语义（"请求"/"Token"）。 */
  centerLabel?: string;
  /** 圆心中央的主数值。 */
  centerValue?: string;
  /** 图区尺寸（宽高一致，保证是正圆）。 */
  size?: number;
  className?: string;
  emptyText?: string;
  /** 清单右侧列表头（默认"占比"）。 */
  trailingHeader?: string;
};

/**
 * 右侧数值清单的一行：`色点 | 名称 | 数值 | 占比`。
 *
 * 三列固定宽度并右对齐：数值长短不一时仍然逐行对齐，
 * 扫视一列数字比逐行找位置快得多。
 */
const LegendRows = ({
  data,
  total,
}: {
  data: DonutDatum[];
  total: number;
}) => (
  <ul className="flex min-w-0 flex-col gap-0.5">
    {data.map((item) => (
      <li
        key={item.name}
        className="flex items-center gap-2 rounded-md px-1.5 py-1 text-xs hover:bg-muted/50"
      >
        <span
          className="size-2 shrink-0 rounded-full"
          style={{ backgroundColor: item.color }}
          aria-hidden="true"
        />
        <span className="min-w-0 flex-1 truncate text-muted-foreground">
          {item.name}
        </span>
        <span className="w-12 shrink-0 text-right tabular-nums">
          {item.value.toLocaleString("zh-CN")}
        </span>
        <span className="w-12 shrink-0 text-right tabular-nums text-muted-foreground">
          {total > 0 ? `${((item.value / total) * 100).toFixed(0)}%` : "0%"}
        </span>
      </li>
    ))}
  </ul>
);

/**
 * 环形图 + 右侧数值清单。
 *
 * 为什么把图例改成左右布局而不是放在图下面：
 *   · 竖排图例在窄容器里会占掉一半高度，饼图被压成一条扁椭圆；
 *   · 用户真正要读的是"每一项占多少"，右对齐的两列数字比
 *     图例色块更容易逐行比较。
 * 左图右表也让卡片在栅格里显得更饱满，而不是上面一个圆下面一堆字。
 */
export const DonutWithLegend = ({
  data,
  centerLabel,
  centerValue,
  size = 148,
  className,
  emptyText = "暂无数据",
  trailingHeader = "占比",
}: DonutWithLegendProps) => {
  const total = data.reduce((sum, item) => sum + item.value, 0);

  if (data.length === 0) {
    return (
      <div
        className={cn(
          "flex items-center justify-center rounded-lg border border-dashed text-xs text-muted-foreground",
          className,
        )}
        style={{ minHeight: size }}
      >
        {emptyText}
      </div>
    );
  }

  return (
    <div className={cn("flex min-w-0 items-center gap-4", className)}>
      {/* 左：环图。固定像素尺寸，避免 flex 把它压成椭圆。 */}
      <div className="relative shrink-0" style={{ width: size, height: size }}>
        {/*
          用确定的 `width`/`height` 而不是 `ResponsiveContainer`：
          后者在本应用的滚动容器里会塌成 7×7 的"图钉"（图渲染了但看不见），
          环形图因此只剩一个色点。固定尺寸下 recharts 一定能算对半径。
        */}
        <PieChart width={size} height={size}>
          <Tooltip
            content={
              <ChartTooltip
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
            innerRadius={size * 0.31}
            outerRadius={size * 0.46}
            paddingAngle={2}
            strokeWidth={0}
            isAnimationActive={false}
          >
            {data.map((item) => (
              <Cell key={item.name} fill={item.color} />
            ))}
          </Pie>
        </PieChart>

        {centerValue ? (
          <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
            <span className="text-lg leading-6 font-semibold tabular-nums">
              {centerValue}
            </span>
            {centerLabel ? (
              <span className="text-[11px] text-muted-foreground">{centerLabel}</span>
            ) : null}
          </div>
        ) : null}
      </div>

      {/* 右：数值清单 */}
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex items-center gap-2 px-1.5 text-[10px] text-muted-foreground">
          <span className="size-2 shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1">名称</span>
          <span className="w-12 shrink-0 text-right">数量</span>
          <span className="w-12 shrink-0 text-right">{trailingHeader}</span>
        </div>
        <LegendRows data={data} total={total} />
      </div>
    </div>
  );
};

/** 单独导出清单，便于自定义布局（例如放在表格里）。 */
export const DonutLegendList = LegendRows;
