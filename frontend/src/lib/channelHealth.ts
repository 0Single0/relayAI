/**
 * 渠道健康的**分档与配色**：口径与色值参考 sub2api 的 `channel-monitor-v2`
 * （`monitorFormat.ts` 的 `errorRateScore` / `scoreToBand`，以及
 * `RelayPulseMatrix.vue` 里那 11 档 green→yellow→red）。
 *
 * 为什么不能再用 99% / 90% 这两刀：
 * 探活是**低频自费动作**，一次 5xx 就会把 7 天窗口的可用率压到 99% 以下。
 * 早先 `successRate < 0.99` 就判「波动」、`< 0.9` 判「降级」，于是
 * "跑得好好的分组"整片发黄，颜色不再传递信息，只剩焦虑。
 *
 * sub2api 的口径是**按错误率线性映射到 0–100 分**：
 *   · 错误率 0% → 100 分；
 *   · 错误率 ≥ 20%（critical）→ 0 分；
 *   · 中间线性插值。
 * 再按 10 分一档落到 11 个色阶上。这样黄色从 80 分以下才开始出现，
 * 绿色有 4 档深浅可以表达"好，但没那么好"。
 */

/** 错误率的满格（critical）阈值：与 sub2api 的 `CriticalErrorRate` 一致。 */
export const HEALTH_CRITICAL_ERROR_RATE = 0.2;

/**
 * 成功率 → 0–100 健康分。
 *
 * `null`（没探过）必须保持 `null`：它和 0 分是两件事 ——
 * 前者是"没有证据"，后者是"证据表明很糟"。
 */
export const healthScoreOf = (successRate: number | null): number | null => {
  if (successRate === null || Number.isNaN(successRate)) {
    return null;
  }
  const errorRate = Math.min(1, Math.max(0, 1 - successRate));
  if (errorRate <= 0) {
    return 100;
  }
  if (errorRate >= HEALTH_CRITICAL_ERROR_RATE) {
    return 0;
  }
  return 100 * (1 - errorRate / HEALTH_CRITICAL_ERROR_RATE);
};

/** 健康分 → 0–10 色阶（11 档）。 */
export const healthBandOf = (score: number): number =>
  Math.max(0, Math.min(10, Math.round(score / 10)));

/**
 * 三档结论。
 *
 * 分界用 80 / 50（sub2api 的 `scoreBand`）：80 分 ≈ 96% 成功率，
 * 50 分 ≈ 90% 成功率。徽标只有三档是因为它要**一眼可读**；
 * 中间那些细微差别交给百分比上的颜色去表达。
 */
export type ChannelHealthLevel = "healthy" | "warning" | "critical";

export const healthLevelOf = (score: number): ChannelHealthLevel => {
  if (score >= 80) {
    return "healthy";
  }
  if (score >= 50) {
    return "warning";
  }
  return "critical";
};

/**
 * 11 档文字色。
 *
 * 写成**字面量**类名而不是 `text-[var(--health-${band})]`：
 * Tailwind 只扫描源码里的静态字符串，模板拼接出来的类名不会进产物 ——
 * 那样颜色会在构建后静默消失（比写错还难查）。
 */
const BAND_TEXT_CLASS: Record<number, string> = {
  0: "text-[var(--health-0)]",
  1: "text-[var(--health-1)]",
  2: "text-[var(--health-2)]",
  3: "text-[var(--health-3)]",
  4: "text-[var(--health-4)]",
  5: "text-[var(--health-5)]",
  6: "text-[var(--health-6)]",
  7: "text-[var(--health-7)]",
  8: "text-[var(--health-8)]",
  9: "text-[var(--health-9)]",
  10: "text-[var(--health-10)]",
};

const BAND_BG_CLASS: Record<number, string> = {
  0: "bg-[var(--health-0)]",
  1: "bg-[var(--health-1)]",
  2: "bg-[var(--health-2)]",
  3: "bg-[var(--health-3)]",
  4: "bg-[var(--health-4)]",
  5: "bg-[var(--health-5)]",
  6: "bg-[var(--health-6)]",
  7: "bg-[var(--health-7)]",
  8: "bg-[var(--health-8)]",
  9: "bg-[var(--health-9)]",
  10: "bg-[var(--health-10)]",
};

/** 健康分 → 文字色类；没分（未探过）用中性灰，不染成红。 */
export const healthTextClassOf = (score: number | null): string =>
  score === null ? "text-muted-foreground" : BAND_TEXT_CLASS[healthBandOf(score)];

/** 一次探活的状态 → 底色类（通过 / 限流 / 失败三档）。 */
export const probeStatusClassOf = (status: string): string => {
  if (status === "ok") {
    // 通过用深绿（第 9 档），而不是最亮的绿色：
    // 一排 60 格里，最亮的绿色在浅色卡片上反而显得发灰。
    return BAND_BG_CLASS[9];
  }
  if (status === "limited") {
    return BAND_BG_CLASS[5];
  }
  return BAND_BG_CLASS[0];
};
