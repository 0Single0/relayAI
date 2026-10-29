import { useMemo } from "react";
import { RefreshCw } from "lucide-react";
import type { Account } from "@/api/types";
import { MyBox } from "@/components/common/MyBox";
import { Button } from "@/components/ui/button";
import { AccountModelCard } from "./AccountModelCard";
import { activeLimits } from "./modelCaps";
import { useLiveAccountModels } from "./useLiveAccountModels";

export const WorkbuddyModelList = ({ account }: { account: Account }) => {
  const { data, isLoading, error, refresh } = useLiveAccountModels(account.id);
  const counts = useMemo(() => {
    const models = data?.models ?? [];
    return {
      free: models.filter(model => model.chargeType === "free").length,
      credits: models.filter(model => model.chargeType === "credits").length,
      unknown: models.filter(model => model.chargeType === "unknown").length,
    };
  }, [data]);
  const now = data?.fetchedAtMs ?? 0;
  const limits = activeLimits(account, now);

  return (
    <div className="flex min-h-0 flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-xs text-muted-foreground" aria-live="polite">
          {data ? <>
            <p>WorkBuddy {data.realm === "cn" ? "国内版" : "国际版"} · 共 {data.models.length} 个</p>
            <p className="mt-1">免费 {counts.free} · 扣积分 {counts.credits} · 未知 {counts.unknown}</p>
          </> : <p>{isLoading ? "正在查询 WorkBuddy 官方模型与积分规则" : "尚未获取到当前扣费规则"}</p>}
        </div>
        <Button size="sm" variant="outline" disabled={isLoading} onClick={() => void refresh()}>
          <RefreshCw aria-hidden="true" />刷新规则
        </Button>
      </div>
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        积分倍率由 WorkBuddy 返回，实际扣分取决于请求用量。弹窗打开期间每 30 秒重新查询，回到窗口时也会刷新。
      </p>
      <MyBox isLoading={isLoading} text="正在查询官方扣费规则..." className="min-h-28">
        {error ? (
          <div role="alert" className="flex flex-col items-start gap-2 rounded-md border p-3 text-xs">
            <p className="text-destructive">查询失败：{error}</p>
            <p className="text-muted-foreground">本次无法确认模型和扣费规则，请重试。</p>
            <Button size="sm" variant="outline" onClick={() => void refresh()}>重新查询</Button>
          </div>
        ) : data?.models.length === 0 ? (
          <p className="py-6 text-center text-xs text-muted-foreground">当前账号没有可展示的模型，请检查账号的模型白名单。</p>
        ) : data ? (
          <div className="grid max-h-[50dvh] grid-cols-[repeat(auto-fill,minmax(min(100%,240px),1fr))] gap-2 overflow-y-auto">
            {data.models.map(model => <AccountModelCard key={model.id}
              model={{ name: model.id, contextWindow: model.contextWindow, maxOutput: model.maxOutput, capabilities: model.capabilities }}
              billing={model} now={now} untilMs={limits.find(limit => limit.model === model.id)?.untilMs} />)}
          </div>
        ) : null}
      </MyBox>
      {data ? <p className="text-[11px] text-muted-foreground">查询于 {new Date(data.fetchedAtMs).toLocaleTimeString("zh-CN", { hour12: false })} · WorkBuddy 官方当前返回值</p> : null}
      <p className="text-[11px] leading-relaxed text-muted-foreground">峰谷时段或优惠以官方当前规则为准；上游未返回的时段信息不作推算。</p>
    </div>
  );
};
