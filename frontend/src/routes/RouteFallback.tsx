import { Suspense, type ReactNode } from "react";
import { PageFallback } from "@/components/common/PageFallback";

/**
 * 给路由元素包一层 Suspense。
 *
 * 本文件**只导出 `routeFallback` 这一个普通函数**，加载态组件放在
 * `components/common/PageFallback`，避免"组件 + 非组件"混合导出导致
 * Fast Refresh 对 routes 目录失效。
 *
 * 加载态里不放数据请求：懒加载期间页面代码还没下载完，数据请求属于页面内部。
 */
export const routeFallback = (element: ReactNode) => (
  <Suspense fallback={<PageFallback />}>{element}</Suspense>
);
