import type { Account } from "@/api/types";
import { useState } from "react";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";
import { formatRemaining } from "@/pages/Accounts/components/formatRemaining";

/**
 * 账号状态。
 *
 * **只看账号本身是否可用**，不把模型级限流算进"不可用"。
 *
 * 为什么要这样分：上游按模型限额是常态（WorkBuddy 返回 6004），
 * 这时账号跑别的模型完全正常。若把它显示成"冷却中"，
 * 用户会以为号废了，去删号重建 —— 反而丢掉配置和历史用量归属。
 *
 * 模型级限流单独一列（`AccountModelCell`）展示，两件事分开说。
 */

/**
 * `now` 由调用方传入而不是内部 `Date.now()`：
 * 渲染期取当前时间是"不纯"的，同一份 props 两次渲染可能得出不同结果。
 */
const describe = (
  account: Account,
  now: number,
): { tone: StatusTone; label: string; hint: string } => {
  if (!account.enabled) {
    return { tone: "neutral", label: "已停用", hint: "账号已停用，不参与调度" };
  }

  const runtime = account.runtime;
  if (!runtime) {
    return { tone: "neutral", label: "状态未知", hint: "网关不可用，读不到运行态" };
  }

  if (runtime.status === "no_credential") {
    return { tone: "warning", label: "无凭据", hint: "缺少凭据，该账号无法发出请求" };
  }

  if (runtime.status === "cooldown") {
    const until = runtime.cooldownUntil
      ? new Date(runtime.cooldownUntil).getTime() - now
      : 0;
    const text =
      until > 0 ? `冷却中 ${formatRemaining(until)}` : "冷却中";
    return {
      tone: "warning",
      label: text,
      hint: "整号被上游拒绝（不是某个模型），冷却结束自动恢复",
    };
  }

  return { tone: "success", label: "可用", hint: "账号可用，可正常参与调度" };
};


export const AccountStatusCell = ({ account }: { account: Account }) => {
  const [now] = useState(() => Date.now());
  const { tone, label, hint } = describe(account, now);

  return (
    <span title={hint}>
      <StatusBadge tone={tone} label={label} />
    </span>
  );
};
