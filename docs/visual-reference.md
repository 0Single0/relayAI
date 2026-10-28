# 视觉参考登记

> 按 AGENTS.md 要求记录每个视觉来源的角色、作用域、复用方式与批准状态。

---

## 已批准的参考

| 字段 | 内容 |
| --- | --- |
| **来源** | FastGPT — `E:\FastGPT-main\packages\web\styles\theme.ts` |
| **选择方式** | 用户明确指定（2026-10-21） |
| **角色** | 配色、字体、圆角、阴影、控件高度的**唯一视觉权威** |
| **作用域** | `frontend/src/styles/index.css` 的主题 token；以及基于这些 token 的组件样式 |
| **批准状态** | ✅ 已批准 |
| **复用方式** | **只取设计 token，不复制代码**。theme.ts 是 Chakra UI 的主题配置，本项目建设在 Tailwind 4 + shadcn/ui 上，配置形态完全不同；提取的是其中的色值、字阶、间距、半径、阴影数值。 |

---

## 登录页（2026-10-27）

| 字段 | 内容 |
| --- | --- |
| **来源** | 用户提供的两张截图 + FastGPT 线上登录页（`https://cloud.fastgpt.cn/login`）及用户转述的登录卡 computed CSS |
| **选择方式** | 用户明确指定，并在迭代中逐项给出尺寸（宽 540 / 内边距 80 / 最小高 600 / 圆角 16 / 输入框高 40） |
| **角色** | **仅 `/login` 页面**的版式、弥散渐变氛围、卡片尺寸与控件高度 |
| **作用域** | `frontend/src/pages/Auth/**`，以及 `styles/index.css` 中的 `--auth-*` token 与 `.auth-*` 规则 |
| **批准状态** | ✅ 已批准 |
| **复用方式** | 只取尺寸与观感：540px 卡片、80px 内边距、16px 圆角、40px 输入框、柔和的双层投影、四角弥散光斑。**不复制**其 logo、wordmark、微信登录、语言切换、服务协议文案 —— 本项目没有对应能力，不画不存在的入口 |

### 配色（用户指定）

| token | 值 | 用途 |
| --- | --- | --- |
| `--auth-blue` | `#598BFD` | 右上与中心冷色光斑；品牌图标渐变起点 |
| `--auth-mint` | `#9AD4CB` | 左上、右下光斑；图标渐变过渡 |
| `--auth-teal` | `#35B3C9` | 左下光斑；图标渐变终点；输入框焦点色 |

> 这组颜色**只作用于登录页背景与焦点态**，不修改管理台的 `--primary #3370FF`。
> 渐变是「蓝 → 薄荷 → 青」，不含紫色，因此不违反 AGENTS.md 对"蓝紫渐变"的限制；
> 光斑由多层 `radial-gradient` 自渐隐实现，没有用 `filter: blur()`（避免大面积合成开销）。

### 尺寸

| 项 | 值 |
| --- | --- |
| 卡片宽度 | `max-width: 540px`（`width: 100%` 兜底窄屏） |
| 卡片最小高度 | `600px`（仅 ≥900px 生效；注册模式靠 `min-height` 自然长高，不被内容顶破） |
| 卡片内边距 | `80px`（≥900px）；窄屏 `clamp(1.75rem, 7vw, 2.75rem)` |
| 圆角 | `16px` |
| 输入框 / 按钮高度 | `40px` / `48px` |
| 阴影 | `rgb(30 64 175 / .1) 0 16px 40px, rgb(15 23 42 / .06) 0 1px 3px` |

---

### 明确不在授权范围内

| 项目 | 说明 |
| --- | --- |
| 布局 | 参考项目不是 `Aside + Header + Main`，本项目的布局由 AGENTS.md 规定，**不因参考而改变** |
| 组件实现 | 不使用 FastGPT 的 React 组件；shadcn/ui 原语保持不变 |
| 品牌资产 | 不使用其 logo、wordmark、图标、插画、文案 |
| 字体文件 | 只沿用其 font-family 声明（系统字体栈），不引入其字体资源 |
| 深色主题 | 参考项目**没有深色模式**。`index.css` 里 `.dark` 的取值由同一套色阶派生，属于本项目产出，不是参考项目的既有实现 |

---

## 提取的 token

### 颜色

| 语义 | 值 | 来源 |
| --- | --- | --- |
| `--primary` | `#3370FF` | `primary.600` |
| `--primary-foreground` | `#FFFFFF` | 按钮前景 |
| `--background` | `#FFFFFF` | 全局白底 |
| `--foreground` | `#1D2532` | `myGray.800` |
| `--muted` | `#F7F8FA` | `myGray.50` |
| `--muted-foreground` | `#667085` | `myGray.500` |
| `--accent` | `#F0F4FF` | `primary.50`（hover 底色） |
| `--accent-foreground` | `#2B5FD9` | `primary.700` |
| `--border` | `#E8EBF0` | `borderColor.low` |
| `--border-strong` | `#C4CBD7` | `borderColor.high` |
| `--ring` | `#94B5FF` | `primary.300`，对齐 `shadowLight` 的 15% 蓝 |
| `--destructive` | `#D92D20` | `red.600` |
| `--success` | `#039855` | `green.600` |
| `--warning` | `#F79009` | `yellow.500` |
| `--sidebar` | `#FBFBFC` | `myGray.25` |

图表色板：`#3370FF`（primary）、`#8774EE`（adora.500）、`#FDB022`（yellow.400）、`#12B76A`（green.500）、`#8A95A7`（myGray.400）。

> **注意**：`success` 用的是绿色，符合参考项目。AGENTS.md 禁止"默认把绿色当主题色"，这里绿色只是语义成功色，主色仍是蓝色 `#3370FF`，不违反。

### 字体

```css
-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue",
Arial, "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "微软雅黑",
sans-serif
```

**这一项改变了原有配置**：之前用 `@fontsource-variable/geist` 的 Web 字体，改为系统字体栈。理由：
1. 参考项目的字体权威是系统栈；
2. 中文界面下 Geist 只覆盖拉丁字符，中英文混排的字重/字宽会不一致；
3. 去掉一个字体依赖，首屏更快。

`@fontsource-variable/geist` 依赖仍在 `package.json` 里，**可以后续移除**（当前不引用，不影响构建）。

### 尺寸

| 项 | 值 | 来源 |
| --- | --- | --- |
| 正文基准 | `14px` / 行高 `1.5` | `fontSizes.sm` = `0.875rem` |
| 圆角基准 | `--radius: 0.5rem` | `radii.md` |
| 按钮高度 | 24 / 30 / 34 / 40px | `Button sizes`（xs / sm / md / lg） |
| 输入框高度 | 32 / 36 / 40px | `Input sizes`（sm / md / lg） |
| 表格行高 | 32px | `Table sizes.md` |
| 阴影 | 4 级，统一 `rgba(19, 51, 107, 0.05~0.2)` | `shadows.1~4` |

### 阴影的用法约束

参考项目的阴影带一层浅蓝底色（`rgb(19 51 107)`），比纯黑阴影"轻"。本项目沿用这一点，但**限制在三级以内**（`--shadow-1` ~ `--shadow-3`），`--shadow-4` 只用于浮层（Dialog / Popover）。AGENTS.md 要求"避免重阴影"，这条约束比参考项目更严。

---

## 与现有实现的关系

| 项 | 改动 |
| --- | --- |
| 主题色 | 中性灰（`oklch(0.205 0 0)`）→ 品牌蓝 `#3370FF` |
| 字体 | Geist Variable → 系统字体栈 |
| 色空间 | `oklch` → `hex`（参考项目的值都是 hex，直接对齐便于比对） |
| 滚动条 | 从 `foreground 18%` 改为 `muted-foreground 32%`，在灰底上更清晰 |
| 焦点环 | 保留 `:focus-visible` 的 2px outline（参考项目全局关掉了，本项目不跟） |

---

## 待补充

如果后续有参考项目的**页面截图**，可以进一步校准：

- 卡片内的间距与分组节奏
- 表头与单元格的具体内边距
- 空状态、加载态、错误态的视觉语言
- 侧边栏选中态的样式（当前按 `primary.50` + `primary.700` 推定）

在此之前，以上 token 是唯一的视觉依据；页面级实现只允许使用这套 token，不得引入新的强调色。
