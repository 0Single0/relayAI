import openaiLogo from "@/assets/providers/openai.svg";
import anthropicLogo from "@/assets/providers/anthropic.svg";
import deepseekLogo from "@/assets/providers/deepseek.svg";
import googleLogo from "@/assets/providers/google.svg";
import alibabaLogo from "@/assets/providers/alibaba.svg";
import zhipuLogo from "@/assets/providers/zhipu.svg";
import moonshotLogo from "@/assets/providers/moonshot.svg";
import minimaxLogo from "@/assets/providers/minimax.svg";
import tencentLogo from "@/assets/providers/tencent.svg";
import { cn } from "@/lib/utils";
import type { ReactNode } from "react";

const logos: Record<string, { src: string; monochrome?: boolean }> = {
  openai: { src: openaiLogo, monochrome: true },
  anthropic: { src: anthropicLogo, monochrome: true },
  deepseek: { src: deepseekLogo },
  google: { src: googleLogo },
  alibaba: { src: alibabaLogo },
  zhipu: { src: zhipuLogo },
  moonshot: { src: moonshotLogo, monochrome: true },
  minimax: { src: minimaxLogo },
  tencent: { src: tencentLogo },
};

/** Brand artwork is vendored unchanged; adjacent text provides its accessible name. */
export const ProviderLogo = ({ provider, className }: { provider: string; className?: string }) => {
  const logo = logos[provider];
  if (!logo) return null;

  return (
    <img
      src={logo.src}
      alt=""
      aria-hidden="true"
      width={18}
      height={18}
      className={cn("size-[18px] shrink-0 object-contain", logo.monochrome && "dark:invert", className)}
    />
  );
};

export const ProviderLabel = ({ provider, children, className }: { provider: string; children: ReactNode; className?: string }) => (
  <span className={cn("inline-flex min-w-0 max-w-full items-center gap-2 align-middle", className)}>
    <ProviderLogo provider={provider} />
    <span className="truncate">{children}</span>
  </span>
);
