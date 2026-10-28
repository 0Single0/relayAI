import { MyBox } from "@/components/common/MyBox";

/**
 * 懒加载页面的占位内容。
 *
 * 单独成组件文件（`components/common/`）而不是写在 `routes/RouteFallback.tsx`
 * 里，是为了让那个文件只导出普通函数 `routeFallback`，不混入组件导出 ——
 * 混合导出会让 Fast Refresh 对整个 routes 目录失效。
 */
export const PageFallback = () => (
  <div className="mx-auto w-full max-w-[1400px] px-4 py-6 sm:px-6">
    <MyBox isLoading text="页面加载中..." className="min-h-[40vh]" />
  </div>
);
