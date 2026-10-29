import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export type AuthCollapseProps = {
  /** 是否展开。收起时内容仍在 DOM 里，下次展开才有过渡可播。 */
  open: boolean;
  children: ReactNode;
  /**
   * 内层额外样式，通常是 `pt-6` 这类间距。
   *
   * **间距必须写在这里，不能交给父级 `gap` / `space-y`**：收起时整块
   * 高度归零，但父级给兄弟节点的 `gap` 仍然生效，会留下一段
   * 看不见却占位的空白，卡片就缩不到位。
   */
  innerClassName?: string;
};

/**
 * 登录 / 注册之间「只在一边出现」的块：昵称字段、记住密码、密码提示与错误。
 *
 * 高度动画的实现（styles/index.css 的 `.auth-collapse`）：
 * `interpolate-size: allow-keywords` + `height: 0 ⇄ auto` 过渡。
 * `auto` 从此成为可插值的关键字，块高从 0 平滑长到内容真实高度 ——
 * 不猜上限、不量 DOM，卡片是内容驱动的 auto 高度，自然跟着平滑涨落。
 *
 * 不用 `grid-template-rows: 0fr ⇄ 1fr`：实测 Chromium 156 上，过渡一旦
 * 启动，fr 的插值结果会被钉死在 0（把 transition 摘掉行高立刻恢复），
 * 展开态直接渲染成 0 高度 —— 字段整个消失。`interpolate-size` 没有这个坑。
 *
 * 兼容性：`interpolate-size` 目前是 Chromium 129+；不支持的浏览器里
 * `height: auto` 与 `0` 之间不能插值，表现为瞬间切换 —— 布局依旧正确，
 * 只是少了动画，属于可接受的渐进增强。
 *
 * 为什么不用 `max-height`：它得先猜一个上限，缓动会被「上限」和「实际高度」
 * 两个数掰弯 —— 矮块前快后慢、高块慢半拍；上限猜小了会裁内容，猜大了
 * 收起后仍留一截空白。
 *
 * 裁剪与聚焦光环的取舍写在 `styles/index.css` 的 `.auth-collapse > *`：
 * `overflow: clip` + `overflow-clip-margin: 8px`。之前用 `overflow: hidden`
 * 把字段聚焦时的 ring 切掉了一圈（用户截图反馈的就是这个）。
 *
 * `inert`：收起时内容还在 DOM 里（高度为 0），必须同时移出焦点顺序与
 * 无障碍树，否则键盘 Tab 会停在看不见的输入框上。
 */
export const AuthCollapse = ({
  open,
  children,
  innerClassName,
}: AuthCollapseProps) => (
  <div
    className="auth-collapse"
    data-open={open ? "true" : "false"}
    inert={open ? undefined : true}
  >
    {/*
      间距层：`innerClassName` 的间距必须落在**裁剪层的后代**上，
      收起时才跟着内容一起归零；挂在裁剪层自身，`pt-6` 会把高度撑住，
      卡片就缩不到位（实测收起后剩 24px）。
    */}
    <div className={cn(innerClassName)}>{children}</div>
  </div>
);
