/**
 * 登录页的弥散渐变背景。
 *
 * 纯装饰：整层 `aria-hidden`，不参与 Tab 顺序，也不接收指针事件。
 * 光斑的漂移在 `prefers-reduced-motion: reduce` 下由全局样式停掉
 * （见 `src/styles/index.css`）—— 这里不再重复写媒体查询。
 */
export const AuthBackdrop = () => (
  <div className="auth-canvas absolute inset-0 overflow-hidden" aria-hidden="true">
    <div className="auth-glow auth-glow--core" />
    <div className="auth-glow auth-glow--mint" />
    <div className="auth-glow auth-glow--blue" />
    <div className="auth-glow auth-glow--teal" />
    <div className="auth-glow auth-glow--sky" />
    <div className="auth-grain" />
  </div>
);
