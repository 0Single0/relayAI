import { Workflow } from "lucide-react";

/**
 * 登录页品牌行。
 *
 * 图标沿用侧边栏的 `Workflow`，避免"登录页一套标、登录后另一套标"的割裂；
 * 只在登录页把它做成渐变底 + 高光，作为品牌露出（配色取自用户参考图）。
 */
export const AuthBrand = () => (
  <div className="flex items-center justify-center gap-3.5">
    <span className="relative flex size-14 shrink-0 items-center justify-center overflow-hidden rounded-[17px] bg-[linear-gradient(140deg,var(--auth-blue)_0%,#4f9ff4_52%,var(--auth-teal)_100%)] text-white shadow-[0_12px_26px_-14px_rgb(51_112_255/0.9)]">
      <span
        className="absolute inset-0 bg-[linear-gradient(180deg,rgb(255_255_255/0.28)_0%,rgb(255_255_255/0)_55%)]"
        aria-hidden="true"
      />
      <Workflow className="relative size-[26px]" aria-hidden="true" />
    </span>
    <span className="flex flex-col">
      <h1
        id="auth-brand-title"
        className="text-[22px] leading-7 font-semibold text-foreground"
      >
        relayAI
      </h1>
      <span className="mt-0.5 text-xs leading-4 text-muted-foreground">
        AI 反向代理管理台
      </span>
    </span>
  </div>
);
