/**
 * 登录页品牌行。
 *
 * 图标与侧边栏共用 `public/brand/logo.png`（用户提供的项目 logo），
 * 不再用 lucide 图标 + 渐变底：那是没有品牌资产时的占位做法。
 * logo 自带蓝→青渐变，白丝带是图片里的负空间，因此这里不加底色容器，
 * 只按高度约束等比缩放（原图约 4:3，宽高都给死会把它压扁）。
 */
export const AuthBrand = () => (
  <div className="flex items-center justify-center gap-3.5">
    <img
      src="/brand/logo.png"
      alt=""
      aria-hidden="true"
      className="h-12 w-auto shrink-0 object-contain"
    />
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
