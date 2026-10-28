/**
 * 图表用的纯函数工具。
 *
 * 与组件分开放：`components/common/Charts/index.tsx` 只导出组件，
 * 混入普通函数会让该文件的 Fast Refresh 失效（组件与工具混导出的规则）。
 */

/** 数值轴 / 指标卡：大数字用 K/M 缩写，否则 1000000 会把纵轴挤爆。 */
export const compactNumber = (value: number) => {
  if (Math.abs(value) >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)}M`;
  }
  if (Math.abs(value) >= 1_000) {
    return `${(value / 1_000).toFixed(1)}K`;
  }
  return String(value);
};

/** 毫秒时延：超过 1 秒换成秒，否则三位数毫秒读起来很累。 */
export const formatLatency = (value: number | null) => {
  if (value === null) {
    return "—";
  }
  return value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${value}ms`;
};

/** 百分比：`null` 表示没测过，必须显示占位而不是 0%。 */
export const formatPercent = (value: number | null, digits = 2) =>
  value === null ? "—" : `${(value * 100).toFixed(digits)}%`;

/** 后端返回的一个时间桶。 */
export type RawSeriesPoint = {
  bucket: string;
  requests: number;
  totalTokens: number;
  upstreamCost: number;
  promptTokens: number;
  completionTokens: number;
  cachedTokens: number;
  avgTtftMs: number | null;
  avgLatencyMs: number | null;
  successRate: number;
  failed: number;
};

export type SeriesPoint = RawSeriesPoint & { label: string };

/**
 * 把稀疏的时间序列补成连续的时间轴。
 *
 * **为什么必须补**：PG 的 `GROUP BY date_trunc(...)` 只会返回**有数据的**桶。
 * 昨天 19:00、20:00 有请求、今天 10:00、11:00 有请求，其余 20 个小时
 * 没有请求时，接口就只给 4 个点。前端直接画这 4 个点，recharts 会把它们
 * **等距铺满整个横轴**，看起来像"19:00 之后就是 20:00，然后直接跳到 10:00"，
 * 中间那段空白被压缩成零宽度。用户看到的就是"图很稀疏、像坏了"。
 *
 * 补 0 之后：横轴是真正的时间轴，空白时段显示为 0，
 * 面积图的凹陷和柱状图的间隔都变成真实信息（"那段时间没人用"）。
 *
 * @param points  后端返回的稀疏点
 * @param from    区间起点
 * @param to      区间终点
 * @param bucket  hour | day
 */
export const fillSeriesGaps = (
  points: RawSeriesPoint[],
  from: Date,
  to: Date,
  bucket: "hour" | "day",
): SeriesPoint[] => {
  const byKey = new Map<string, RawSeriesPoint>();
  for (const point of points) {
    byKey.set(bucketKey(new Date(point.bucket), bucket), point);
  }

  const result: SeriesPoint[] = [];
  const cursor = truncateToBucket(new Date(from), bucket);
  const end = truncateToBucket(new Date(to), bucket);

  // 上限保护：区间写错（比如 from 传了 1970 年）时不要让浏览器生成几十万个点。
  const maxPoints = bucket === "hour" ? 24 * 40 : 400;
  let guard = 0;

  while (cursor.getTime() <= end.getTime() && guard < maxPoints) {
    const key = bucketKey(cursor, bucket);
    const existing = byKey.get(key);
    const at = new Date(cursor);

    result.push({
      bucket: at.toISOString(),
      requests: existing?.requests ?? 0,
      totalTokens: existing?.totalTokens ?? 0,
      upstreamCost: existing?.upstreamCost ?? 0,
      promptTokens: existing?.promptTokens ?? 0,
      completionTokens: existing?.completionTokens ?? 0,
      cachedTokens: existing?.cachedTokens ?? 0,
      // 空桶的延迟保持 null：0ms 会被画成"快到 0"，
      // 而真相是"这个时段没有请求"，不该有延迟。
      avgTtftMs: existing?.avgTtftMs ?? null,
      avgLatencyMs: existing?.avgLatencyMs ?? null,
      successRate: existing?.successRate ?? 0,
      failed: existing?.failed ?? 0,
      label: formatBucketLabel(at, bucket),
    });

    cursor.setTime(
      bucket === "hour"
        ? cursor.getTime() + 3600_000
        : cursor.getTime() + 86400_000,
    );
    guard += 1;
  }

  return result;
};

/** 桶的标识：小时桶按"年月日时"，天桶按"年月日"，都用本地时间。 */
const bucketKey = (at: Date, bucket: "hour" | "day") => {
  const year = at.getFullYear();
  const month = String(at.getMonth() + 1).padStart(2, "0");
  const day = String(at.getDate()).padStart(2, "0");
  if (bucket === "day") {
    return `${year}-${month}-${day}`;
  }
  return `${year}-${month}-${day} ${String(at.getHours()).padStart(2, "0")}`;
};

/** 把一个时刻对齐到它所在桶的起点。 */
export const truncateToBucket = (at: Date, bucket: "hour" | "day") => {
  const result = new Date(at);
  result.setMinutes(0, 0, 0);
  if (bucket === "day") {
    result.setHours(0);
  }
  return result;
};

/** 横轴标签：小时桶显示 HH:mm，天桶显示 MM/DD。 */
export const formatBucketLabel = (at: Date, bucket: "hour" | "day") =>
  bucket === "hour"
    ? at.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
