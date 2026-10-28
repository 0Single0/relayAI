import { lazy } from "react";

/**
 * 页面懒加载定义。
 *
 * 单独成文件的原因：`lazy()` 的结果是组件，但如果它们和 `router` 导出写在
 * 同一个文件里，Fast Refresh 会认为该文件"既导出组件又导出非组件"，
 * 从而放弃热更新。这里只导出组件映射，`routes/index.tsx` 只导出 router。
 *
 * 代码分割的收益：recharts 与表格页面各自成块，首屏只加载仪表盘需要的部分。
 */
export const DashboardPage = lazy(() =>
  import("@/pages/Dashboard").then((module) => ({ default: module.DashboardPage })),
);

export const ChannelsPage = lazy(() =>
  import("@/pages/Channels").then((module) => ({ default: module.ChannelsPage })),
);

export const ChannelDetailPage = lazy(() =>
  import("@/pages/Channels/ChannelDetail").then((module) => ({
    default: module.ChannelDetailPage,
  })),
);

export const AccountsPage = lazy(() =>
  import("@/pages/Accounts").then((module) => ({ default: module.AccountsPage })),
);

export const AccountDetailPage = lazy(() =>
  import("@/pages/Accounts/AccountDetail").then((module) => ({
    default: module.AccountDetailPage,
  })),
);

export const StatusPage = lazy(() =>
  import("@/pages/Status").then((module) => ({ default: module.StatusPage })),
);

export const TaskChannelPage = lazy(() =>
  import("@/pages/Tasks/TaskChannelPage").then((module) => ({
    default: module.TaskChannelPage,
  })),
);

export const UsagePage = lazy(() =>
  import("@/pages/Usage").then((module) => ({ default: module.UsagePage })),
);

export const KeysPage = lazy(() =>
  import("@/pages/Keys").then((module) => ({ default: module.KeysPage })),
);

export const ModelPlazaPage = lazy(() =>
  import("@/pages/ModelPlaza").then((module) => ({ default: module.ModelPlazaPage })),
);

export const OpsPage = lazy(() =>
  import("@/pages/Ops").then((module) => ({ default: module.OpsPage })),
);

export const ProxiesPage = lazy(() =>
  import("@/pages/Proxies").then((module) => ({ default: module.ProxiesPage })),
);

export const LoginPage = lazy(() =>
  import("@/pages/Auth/LoginPage").then((module) => ({ default: module.LoginPage })),
);

export const NotFoundPage = lazy(() =>
  import("@/pages/NotFound").then((module) => ({ default: module.NotFoundPage })),
);

export const RedeemCodesPage = lazy(() =>
  import("@/pages/RedeemCodes").then((module) => ({ default: module.RedeemCodesPage })),
);

export const SettingsPage = lazy(() =>
  import("@/pages/Settings").then((module) => ({ default: module.SettingsPage })),
);

export const UserCenterPage = lazy(() =>
  import("@/pages/UserCenter").then((module) => ({ default: module.UserCenterPage })),
);

export const UsersPage = lazy(() =>
  import("@/pages/Users").then((module) => ({ default: module.UsersPage })),
);