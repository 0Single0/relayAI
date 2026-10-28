import { createBrowserRouter, Navigate } from "react-router-dom";
import { AppShell } from "@/components/common/AppShell";
import { RedirectIfAuthed, RequireAdmin, RequireAuth } from "@/routes/RequireAuth";
import { routeFallback } from "@/routes/RouteFallback";
import {
  AccountDetailPage,
  AccountsPage,
  ChannelDetailPage,
  ChannelsPage,
  DashboardPage,
  KeysPage,
  ModelPlazaPage,
  OpsPage,
  ProxiesPage,
  LoginPage,
  NotFoundPage,
  RedeemCodesPage,
  SettingsPage,
  StatusPage,
  TaskChannelPage,
  UsagePage,
  UserCenterPage,
  UsersPage,
} from "@/routes/pages";

/**
 * 路由表。**只导出 router**：页面组件在 `routes/pages.ts`，加载态在
 * `routes/RouteFallback.tsx`，这样 Fast Refresh 不会因为
 * "文件同时导出组件与非组件"而失效。
 *
 * 登录页在**外壳之外**：未登录的人看不到侧边栏与顶栏，
 * 有导航反而是误导（点进去全被踢回来）。
 */
const withFallback = routeFallback;

/**
 * 管理员页面：包一层 `RequireAdmin`。
 *
 * 为什么不能只靠"隐藏导航项"：隐藏只是看不见入口，直接敲 URL 照样能进。
 * 后端接口已经会返回 403，但前端也要给出明确的"无权限"页，
 * 而不是让越权的用户看到一堆空表格加报错。
 */
const admin = (element: React.ReactNode) => (
  <RequireAdmin>{withFallback(element)}</RequireAdmin>
);

export const router = createBrowserRouter([
  {
    path: "/login",
    element: (
      <RedirectIfAuthed>
        <LoginPage />
      </RedirectIfAuthed>
    ),
  },
  {
    path: "/plaza",
    element: withFallback(<ModelPlazaPage />),
  },
  {
    path: "/",
    element: (
      <RequireAuth>
        <AppShell />
      </RequireAuth>
    ),
    children: [
      { index: true, element: <Navigate to="/dashboard" replace /> },
      // 普通用户也能看的：仪表盘、我的账户。其余全部要管理员。
      { path: "dashboard", element: withFallback(<DashboardPage />) },
      // 模型广场：外壳内一份（带侧边栏），公开的 /plaza 在外壳之外。
      { path: "models", element: withFallback(<ModelPlazaPage />) },
      { path: "account", element: withFallback(<UserCenterPage />) },
      {
        path: "channels",
        element: admin(<ChannelsPage />),
      },
      { path: "channels/:id", element: admin(<ChannelDetailPage />) },
      { path: "accounts", element: admin(<AccountsPage />) },
      { path: "accounts/:id", element: admin(<AccountDetailPage />) },
      { path: "status", element: admin(<StatusPage />) },
      { path: "ops", element: admin(<OpsPage />) },
      // 任务组没有总览页：`/tasks` 直接跳到第一个有任务能力的渠道。
      // 侧边栏的二级菜单已经列出了所有渠道（见 navigation.ts 的 `dynamic` 标记）。
      // `/tasks` 直接进国内版：任务菜单固定两项，不再由数据重定向。
      { path: "tasks", element: <Navigate to="/tasks/cn" replace /> },
      { path: "tasks/:channelId", element: admin(<TaskChannelPage />) },
      { path: "usage", element: admin(<UsagePage />) },
      { path: "proxies", element: admin(<ProxiesPage />) },
      { path: "keys", element: admin(<KeysPage />) },
      { path: "users", element: admin(<UsersPage />) },
      { path: "redeem-codes", element: admin(<RedeemCodesPage />) },
      { path: "settings", element: admin(<SettingsPage />) },
      { path: "*", element: withFallback(<NotFoundPage />) },
    ],
  },
]);
