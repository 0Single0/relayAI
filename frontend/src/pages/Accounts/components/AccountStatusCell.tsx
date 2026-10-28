import type { Account } from "@/api/types";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";

const describe = (account: Account): { tone: StatusTone; label: string } => {
  if (!account.enabled) {
    return { tone: "neutral", label: "已停用" };
  }

  const runtime = account.runtime;
  if (!runtime) {
    return { tone: "neutral", label: "状态未知" };
  }

  if (runtime.status === "no_credential") {
    return { tone: "warning", label: "无凭据" };
  }

  if (runtime.status === "cooldown") {
    const until = runtime.cooldownUntil
      ? new Date(runtime.cooldownUntil).getTime() - Date.now()
      : 0;
    const minutes = Math.max(0, Math.round(until / 60000));
    return {
      tone: "warning",
      label: minutes > 0 ? `冷却中 ${minutes}m` : "冷却中",
    };
  }

  return { tone: "success", label: "正常" };
};

export const AccountStatusCell = ({ account }: { account: Account }) => {
  const { tone, label } = describe(account);

  return <StatusBadge tone={tone} label={label} />;
};
