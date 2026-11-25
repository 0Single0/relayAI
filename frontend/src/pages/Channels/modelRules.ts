/** Naming suggestions only. Account catalogs determine actual support. */
const families: Record<string, string[]> = {
  openai: ["gpt*", "chatgpt*", "o1*", "o3*", "o4*"],
  anthropic: ["claude*"], deepseek: ["deepseek*"], google: ["gemini*", "gemma*"],
  alibaba: ["qwen*", "qwq*", "qvq*"], zhipu: ["glm*", "chatglm*"],
  moonshot: ["kimi*", "moonshot*"], minimax: ["minimax*"], tencent: ["hunyuan*", "hy-*"],
};

export const recommendedModelRules = (provider: string): string[] =>
  (families[provider] ?? []).flatMap((family) => [family, `*/${family}`]);

export const matchesModelRule = (rule: string, name: string): boolean => {
  const parts = rule.toLowerCase().split("*");
  let rest = name.toLowerCase();
  if (parts.length === 1) return rest === parts[0];
  if (!rest.startsWith(parts[0])) return false;
  rest = rest.slice(parts[0].length);
  for (const part of parts.slice(1, -1)) {
    const at = rest.indexOf(part);
    if (at < 0) return false;
    rest = rest.slice(at + part.length);
  }
  return rest.endsWith(parts[parts.length - 1]);
};
