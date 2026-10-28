import { useEffect, type ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { PageFallback } from "@/components/common/PageFallback";
import { ForbiddenPage } from "@/pages/Forbidden";
import { useAuthStore } from "@/store/auth";

/**
 * 登录守卫。
 *
 * 三种状态必须分清：
 *   1. `resolved === false`（还在问后端）→ **显示加载态**，不要跳登录页。
 *      少了这一步，刷新页面必定闪一下登录页再跳回来。
 *   2. 未登录 → 跳登录页，并把当前位置塞进 `state.from`，
 *      登录成功后回到原来那一页，而不是永远落在仪表盘。
 *   3. 已登录 → 放行。
 */
export const RequireAuth = ({ children }: { children: ReactNode }) => {
  const resolved = useAuthStore((state) => state.resolved);
  const user = useAuthStore((state) => state.user);
  const bootstrap = useAuthStore((state) => state.bootstrap);
  const location = useLocation();

  useEffect(() => {
    if (!resolved) {
      void bootstrap();
    }
  }, [bootstrap, resolved]);

  if (!resolved) {
    return <PageFallback />;
  }

  if (!user) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }

  return <>{children}</>;
};

/** 已登录用户不该再看到登录页。 */
export const RedirectIfAuthed = ({ children }: { children: ReactNode }) => {
  const resolved = useAuthStore((state) => state.resolved);
  const user = useAuthStore((state) => state.user);
  const bootstrap = useAuthStore((state) => state.bootstrap);

  useEffect(() => {
    if (!resolved) {
      void bootstrap();
    }
  }, [bootstrap, resolved]);

  if (!resolved) {
    return <PageFallback />;
  }

  if (user) {
    return <Navigate to="/dashboard" replace />;
  }

  return <>{children}</>;
};

/**
 * 管理员守卫。
 *
 * 为什么必须在**路由层**挡，而不是只隐藏导航项：
 * 隐藏菜单只是"看不见入口"，用户直接敲 URL 照样能进页面、看到全站数据。
 * 真正挡住越权的是后端接口（`requireAdmin` 会返回 403），
 * 但前端也要给一个明确的"无权限"页，而不是让页面显示成一堆空表格加报错。
 *
 * 注意用 403 语义的页面而不是跳回仪表盘：
 * 跳走会让用户以为自己点错了，看不到"这是权限问题"。
 */
export const RequireAdmin = ({ children }: { children: ReactNode }) => {
  const resolved = useAuthStore((state) => state.resolved);
  const user = useAuthStore((state) => state.user);
  const bootstrap = useAuthStore((state) => state.bootstrap);

  useEffect(() => {
    if (!resolved) {
      void bootstrap();
    }
  }, [bootstrap, resolved]);

  if (!resolved) {
    return <PageFallback />;
  }

  if (!user) {
    return <Navigate to="/login" replace />;
  }

  if (user.role !== "admin") {
    return <ForbiddenPage />;
  }

  return <>{children}</>;
};
