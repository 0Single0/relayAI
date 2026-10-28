/**
 * 模型厂商识别 + 官方 logo。
 *
 * **用官方 logo**（Simple Icons 提供的各厂商官方 SVG），不用字母标记 ——
 * 字母标记只是"没有素材时的退路"。
 *
 * **图标放在 `public/logos/` 而不是走 CDN**：
 * 一开始用的是 `cdn.simpleicons.org`，实测浏览器里经常加载不出来
 * （外网 CDN 在国内不稳定），页面上会留一排空白方框 ——
 * 那比没有 logo 更难看。本地文件没有这个问题。
 *
 * 素材来自 Simple Icons 与 thesvg（两者都是各厂商官方 SVG 的汇总），
 * 只保留实际用到的几个。
 * **拿不到官方图标的厂商**用中文/字母标记，不伪造图形：
 *   · 腾讯混元 / 字节 / 百度 —— Simple Icons 与 thesvg 都未收录，用中文单字；
 *   · OpenAI —— 商标已从 Simple Icons 移除（404），用 `AI` 字母。
 * 编一个"看起来像官方"的图标比没有更糟：那是在冒充品牌。
 */

export type Vendor = {
  /** 厂商全名。 */
  label: string;
  /** 图标文件名（不带扩展名）；空串表示没有图标，用文字标记。 */
  slug: string;
  /** 图标扩展名；默认 svg。只有拿到 png/ico 时才需要填。 */
  ext?: string;
  /** 没有图标时的字母标记。 */
  mark: string;
  /** 标记背景（仅无图标时用）。 */
  className: string;
};

/**
 * 厂商识别表。
 *
 * 顺序有意义：`gpt-4o` 与 `gpt-4o-mini` 都归 OpenAI，
 * 而 `o1`/`o3` 这类短名要用单词边界判断，否则 `openrouter` 也会命中。
 */
const VENDORS: Array<{ match: RegExp; vendor: Vendor }> = [
  {
    match: /^(gpt|chatgpt|text-davinci|davinci)/i,
    vendor: { label: "OpenAI", slug: "", mark: "AI", className: "bg-emerald-600/15 text-emerald-700" },
  },
  {
    match: /^o[134](-|$)/i,
    vendor: { label: "OpenAI", slug: "", mark: "AI", className: "bg-emerald-600/15 text-emerald-700" },
  },
  {
    match: /^claude/i,
    vendor: { label: "Anthropic", slug: "anthropic", mark: "C", className: "bg-orange-600/15 text-orange-700" },
  },
  {
    match: /^(gemini|gemma)/i,
    vendor: { label: "Google", slug: "googlegemini", mark: "G", className: "bg-blue-600/15 text-blue-700" },
  },
  {
    match: /^deepseek/i,
    vendor: { label: "DeepSeek", slug: "deepseek", mark: "DS", className: "bg-indigo-600/15 text-indigo-700" },
  },
  {
    match: /^grok/i,
    vendor: { label: "xAI", slug: "x", mark: "X", className: "bg-neutral-600/15 text-neutral-700" },
  },
  {
    match: /^(llama|meta)/i,
    vendor: { label: "Meta", slug: "meta", mark: "L", className: "bg-blue-700/15 text-blue-800" },
  },
  {
    match: /^(mistral|mixtral|codestral)/i,
    vendor: { label: "Mistral", slug: "mistralai", mark: "M", className: "bg-amber-600/15 text-amber-700" },
  },
  {
    match: /^(qwen|qwq)/i,
    vendor: { label: "阿里", slug: "alibabacloud", mark: "Q", className: "bg-purple-600/15 text-purple-700" },
  },
  {
    match: /^(kimi|moonshot)/i,
    vendor: { label: "月之暗面", slug: "moonshotai", mark: "K", className: "bg-sky-600/15 text-sky-700" },
  },
  {
    // 小米 MiMo
    match: /^(mimo|xiaomi)/i,
    vendor: { label: "小米", slug: "xiaomi", mark: "MI", className: "bg-orange-600/15 text-orange-700" },
  },
  {
    match: /^minimax/i,
    vendor: { label: "MiniMax", slug: "minimax", mark: "MM", className: "bg-pink-600/15 text-pink-700" },
  },
  {
    // 智谱 GLM：Simple Icons 未收录，用 thesvg 集合里的官方标志。
    match: /^(glm|chatglm)/i,
    vendor: { label: "智谱", slug: "zhipu", mark: "GLM", className: "bg-violet-600/15 text-violet-700" },
  },
  {
    // 腾讯混元
    match: /^(hy|hunyuan)/i,
    vendor: { label: "腾讯", slug: "", mark: "混", className: "bg-cyan-600/15 text-cyan-700" },
  },
  {
    // 阶跃星辰 StepFun：Simple Icons 未收录，用 thesvg 集合里的官方标志。
    match: /^step/i,
    vendor: { label: "阶跃星辰", slug: "stepfun", mark: "阶", className: "bg-teal-600/15 text-teal-700" },
  },
  {
    match: /^(doubao|skylark)/i,
    vendor: { label: "字节", slug: "", mark: "豆", className: "bg-rose-600/15 text-rose-700" },
  },
  {
    match: /^(ernie|wenxin)/i,
    vendor: { label: "百度", slug: "", mark: "文", className: "bg-blue-600/15 text-blue-700" },
  },
  {
    match: /^fake/i,
    vendor: { label: "链路自测", slug: "", mark: "测", className: "bg-muted text-muted-foreground" },
  },
];

const FALLBACK: Vendor = {
  label: "其他",
  slug: "",
  mark: "?",
  className: "bg-muted text-muted-foreground",
};

/** 按模型名推断厂商；认不出时返回"其他"。 */
export const vendorOf = (model: string): Vendor => {
  for (const entry of VENDORS) {
    if (entry.match.test(model)) {
      return entry.vendor;
    }
  }
  return FALLBACK;
};

/** 官方 logo 的 URL；没有 slug 时返回 null。 */
export const logoUrl = (vendor: Vendor) =>
  vendor.slug ? `/logos/${vendor.slug}.${vendor.ext ?? "svg"}` : null;
