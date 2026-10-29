import type { CatalogModel } from "@/api/catalog";
import type { LiveAccountModel } from "@/api/accounts/liveModels";
import { cn } from "@/lib/utils";
import { compactTokens, modelCapabilities } from "./modelCaps";
import { formatRemaining } from "./formatRemaining";

type ModelInfo = Pick<CatalogModel, "name" | "contextWindow" | "maxOutput" | "capabilities">;

export const AccountModelCard = ({ model, billing, untilMs, now }: {
  model: ModelInfo;
  billing?: LiveAccountModel;
  untilMs?: number;
  now: number;
}) => {
  const limited = untilMs !== undefined && untilMs > now;
  const caps = modelCapabilities(model);

  return (
    <div className={cn("flex min-w-0 flex-col gap-1 rounded-lg border px-2.5 py-2", limited ? "border-warning/40 bg-warning/5" : "bg-card")}>
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <span className="min-w-0 flex-1 break-all font-mono text-xs font-medium">{model.name}</span>
        {limited ? <span className="shrink-0 rounded bg-warning/20 px-1 text-[10px] text-warning">限流 {formatRemaining(untilMs - now)}</span> : null}
      </div>
      <div className="flex flex-wrap items-center gap-1 text-[10px] text-muted-foreground">
        <span title="上下文窗口">上下文 {compactTokens(model.contextWindow)}</span>
        <span aria-hidden="true">·</span>
        <span title="单次最大输出">输出 {compactTokens(model.maxOutput)}</span>
        {caps.map(cap => <span key={cap} className="rounded bg-muted px-1 text-foreground/80">{cap}</span>)}
      </div>
      {billing ? (
        <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px]">
          <span className="rounded bg-muted px-1.5 py-0.5 font-medium">
            {billing.chargeType === "free" ? "免费 · 不扣积分" : billing.chargeType === "credits" ? "扣积分" : "扣费规则未知"}
          </span>
          {billing.chargeType === "credits" ? <span className="tabular-nums">倍率 ×{billing.creditMultiplier}</span> : null}
          {billing.credits ? <span className="break-all text-muted-foreground" title="WorkBuddy 本次返回的原始计费标识">{billing.credits}</span> : null}
          {billing.chargeType === "unknown" ? <span className="text-muted-foreground">上游未提供可识别的积分倍率</span> : null}
        </div>
      ) : null}
    </div>
  );
};
