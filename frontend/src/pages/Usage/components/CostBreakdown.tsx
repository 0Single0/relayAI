import { Info } from "lucide-react";
import type { UsageLog } from "@/api/types";
import { cn } from "@/lib/utils";

/**
 * 费用明细浮层：把"这笔钱怎么算出来的"摊开。
 *
 * 为什么值得单独做一个：
 *   用户看到一条 `$0.009446` 首先会问"凭什么"。只给总额、不给拆分，
 *   他只能去比对别家的账单 —— 一旦对不上就不信任这个系统了。
 *   把「用量 × 单价，再乘倍率」摆出来，他自己就能验算。
 *
 * 关键点：**用当时的快照**，不是今天的价格。
 * 价格会变，用今天的价去算历史费用必然对不上，那比不显示更糟。
 */
export const CostBreakdown = ({ log }: { log: UsageLog }) => {
  const snapshot = log.priceSnapshot;

  if (log.isRejection) {
    return null;
  }

  // 没有快照（老记录）：明说，不要编。
  if (!snapshot) {
    return (
      <span className="group relative inline-flex items-center gap-1">
        <span className="tabular-nums text-[13px] font-medium">${log.billedCost.toFixed(6)}</span>
        <Info className="size-3 text-muted-foreground" aria-hidden="true" />
        <span
          role="tooltip"
          className={cn(
            "pointer-events-none absolute top-full right-0 z-20 mt-1 hidden w-56 rounded-md border",
            "bg-popover p-2 text-[11px] leading-5 text-muted-foreground shadow-md",
            "group-hover:block group-focus-within:block",
          )}
        >
          这条记录早于价格快照功能，当时用的单价没有留存。
          用今天的价格反推会得出和账单不一致的数字，所以这里不给拆分。
        </span>
      </span>
    );
  }

  const cached = Math.min(log.cachedTokens, log.promptTokens);
  const freshInput = Math.max(log.promptTokens - cached, 0);
  // 单位换算：单价是"元 / 百万 token"。
  const inputCost = (freshInput * snapshot.inputPrice) / 1_000_000;
  const cacheCost = (cached * snapshot.cacheReadPrice) / 1_000_000;
  const outputCost = (log.completionTokens * snapshot.outputPrice) / 1_000_000;

  return (
    <span className="group relative inline-flex items-center gap-1">
      <span className="tabular-nums text-[13px] font-medium">${log.billedCost.toFixed(6)}</span>
      <Info className="size-3 text-muted-foreground" aria-hidden="true" />
      <div
        role="tooltip"
        className={cn(
          "pointer-events-none absolute top-full right-0 z-20 mt-1 hidden w-64 rounded-md border",
          "bg-popover p-2.5 text-[11px] leading-5 shadow-md",
          "group-hover:block group-focus-within:block",
        )}
      >
        <p className="mb-1.5 text-xs font-medium">费用明细</p>

        <Row
          label={`输入 ${freshInput.toLocaleString("zh-CN")} token`}
          value={formatMoney(inputCost)}
        />
        <Row
          label={`缓存 ${cached.toLocaleString("zh-CN")} token`}
          value={formatMoney(cacheCost)}
        />
        <Row
          label={`输出 ${log.completionTokens.toLocaleString("zh-CN")} token`}
          value={formatMoney(outputCost)}
        />

        <div className="my-1.5 border-t" />

        <Row label="输入单价" value={`${formatPrice(snapshot.inputPrice)} / 1M token`} muted />
        <Row label="输出单价" value={`${formatPrice(snapshot.outputPrice)} / 1M token`} muted />
        {cached > 0 ? (
          <Row
            label="缓存单价"
            value={`${formatPrice(snapshot.cacheReadPrice)} / 1M token`}
            muted
          />
        ) : null}

        <div className="my-1.5 border-t" />

        <Row label="上游成本" value={formatMoney(log.upstreamCost)} muted />
        <Row
          label="倍率"
          value={`${snapshot.channelRatio.toFixed(2)}× ${snapshot.modelRatio.toFixed(2)}×`}
          muted
        />
        <Row label="实际扣费" value={formatMoney(log.billedCost)} strong />
      </div>
    </span>
  );
};

const Row = ({
  label,
  value,
  muted,
  strong,
}: {
  label: string;
  value: string;
  muted?: boolean;
  strong?: boolean;
}) => (
  <div className="flex items-baseline justify-between gap-3">
    <span className={cn("min-w-0 truncate", muted ? "text-muted-foreground" : "text-foreground")}>
      {label}
    </span>
    <span
      className={cn(
        "shrink-0 tabular-nums",
        strong ? "font-medium text-foreground" : muted ? "text-muted-foreground" : "",
      )}
    >
      {value}
    </span>
  </div>
);

/** 金额：小于 0.01 时保留 6 位，否则 4 位 —— 太长的 0 会让人读不出量级。 */
const formatMoney = (value: number) =>
  value >= 0.01 ? `$${value.toFixed(4)}` : `$${value.toFixed(6)}`;

/** 单价：去掉无意义的尾零。 */
const formatPrice = (value: number) =>
  `$${Number.isInteger(value) ? value : value.toFixed(4)}`;
