import {
  Activity,
  Boxes,
  FileClock,
  Gauge,
  KeyRound,
  ListChecks,
  Network,
  ServerCog,
  type LucideIcon,
  Settings,
  Ticket,
  UserRound,
  Users,
  Wallet,
} from "lucide-react";

export type NavigationItem = {
  label: string;
  to: string;
  icon: LucideIcon;
  end?: boolean;
  /**
   * 仅管理员可见。
   *
   * 这是**第二道**防线：路由层已有 `RequireAdmin` 负责真正的拦截。
   * 这里的作用是别让普通用户看到点了就 403 的入口 ——
   * 菜单上摆一堆进不去的门，比少几个入口更让人困惑。
   */
  adminOnly?: boolean;
};

export type NavigationGroup = {
  label: string;
  items: NavigationItem[];
  /** 整组仅管理员可见（组内全部项都是 adminOnly 时的便捷写法）。 */
  adminOnly?: boolean;
  /**
   * 该组的子菜单由数据驱动（目前只有「任务」）。
   *
   * 用标记而不是"某个 item.to === '/tasks'"来判断：
   * 任务组已经没有总览项了（子菜单直接挂在组下），
   * 靠具体路径判断会让整个二级菜单消失。
   */
};

/**
 * 导航分组（SPEC 8.1）。
 *
 * **菜单项只有图标 + 名称，不带描述**：侧边栏是高频点击的定位工具，
 * 每项多一行说明会让 8 个入口变成 16 行，扫视成本翻倍，而描述本身
 * 在页面标题栏里已经出现过一次（见 `pageDescriptions`）。
 *
 * 「任务」组的二级菜单不写死：它由 `GET /api/v1/tasks/channels` 返回的
 * 有运营能力的渠道决定，在 Sidebar 里动态拼接（见 AppShellWithTasks）。
 */
export const navigationGroups: NavigationGroup[] = [
  {
    label: "概览",
    items: [
      { label: "仪表盘", to: "/dashboard", icon: Gauge },
      { label: "模型广场", to: "/models", icon: Boxes },
    ],
  },
  {
    label: "分组与账号",
    adminOnly: true,
    items: [
      { label: "分组管理", to: "/channels", icon: Boxes },
      { label: "账号管理", to: "/accounts", icon: UserRound },
      { label: "分组状态", to: "/status", icon: Activity },
      { label: "运维监控", to: "/ops", icon: ServerCog },
    ],
  },
  {
    /*
      任务组是**常驻功能入口**，不是分组的附属物。

      之前这里的二级菜单由"有哪些分组"动态派生，于是没建分组时
      整个「任务」菜单会消失 —— 而任务是产品自带的能力，
      不该因为数据没配好就不见了。

      现在固定两项（国内版 / 国际版）：它们对应**上游产品的两个版本**，
      不是用户建的分组。分组只决定用哪个号去执行，不影响这里有没有入口。
    */
    label: "任务",
    adminOnly: true,
    items: [
      { label: "WorkBuddy 国内版", to: "/tasks/cn", icon: ListChecks },
      { label: "WorkBuddy 国际版", to: "/tasks/intl", icon: ListChecks },
    ],
  },
  {
    label: "用量",
    adminOnly: true,
    items: [{ label: "使用记录", to: "/usage", icon: FileClock }],
  },
  {
    label: "系统",
    adminOnly: true,
    items: [
      { label: "代理", to: "/proxies", icon: Network },
      { label: "密钥管理", to: "/keys", icon: KeyRound },
      { label: "用户管理", to: "/users", icon: Users },
      { label: "兑换码", to: "/redeem-codes", icon: Ticket },
      { label: "设置", to: "/settings", icon: Settings },
    ],
  },
  {
    label: "我的",
    items: [{ label: "用户中心", to: "/account", icon: Wallet }],
  },
];

/** 扁平化后的导航项，用于按路径查找页面元信息。 */
export const navigationItems: NavigationItem[] = navigationGroups.flatMap(
  (group) => group.items,
);

export type PageMeta = {
  title: string;
  description: string;
};

/**
 * 页面标题栏的一行说明。
 *
 * 与导航项分开维护：导航项不该带描述（见 `navigationGroups` 注释），
 * 但页面顶部需要一句话交代"这页能干什么"。
 */
const pageDescriptions: Record<string, string> = {
  "/dashboard": "今日用量、延迟与异常",
  "/channels": "计价分组：倍率、优先级与调度权重",
  "/accounts": "跨分组号池与运行态",
  "/status": "每个分组的可用性与最近请求",
  "/ops": "并发水位、账号健康与网关告警",
  "/tasks/cn": "国内版：成长任务、签到、猫猫旅行与抽奖",
  "/tasks/intl": "国际版：每日活跃打卡",
  "/usage": "请求级日志与错误原文",
  "/proxies": "出站代理与出口 IP",
  "/keys": "渠道与模型白名单",
  "/users": "账户、角色与余额",
  "/redeem-codes": "批量生成与核销状态",
  "/account": "余额、兑换与流水",
  "/settings": "网关、任务与日志",
};

/** 详情页等非导航项的手工元信息（导航里没有它们）。 */
const detailPageMeta: Record<string, PageMeta> = {
  "/channels/": { title: "渠道详情", description: "账号池、模型映射与用量" },
  "/accounts/": { title: "账号详情", description: "凭据、额度与最近请求" },
  "/usage/": { title: "请求详情", description: "重试链与错误原文" },
};

const matchItem = (pathname: string): NavigationItem | undefined => {
  const sorted = [...navigationItems].sort((a, b) => b.to.length - a.to.length);

  return sorted.find((item) => {
    // 列表项不应吞掉它的详情子路由：/channels/wb-cn 不是「渠道管理」。
    if (!item.end && pathname.startsWith(`${item.to}/`)) {
      return false;
    }

    return item.end ? pathname === item.to : pathname.startsWith(item.to);
  });
};

export const getPageMeta = (pathname: string): PageMeta => {
  const item = matchItem(pathname);
  if (item) {
    return {
      title: item.label,
      description: pageDescriptions[item.to] ?? "",
    };
  }

  if (pathname.startsWith("/tasks/")) {
    return { title: "渠道任务", description: "账号任务与每日动作执行" };
  }

  for (const [prefix, meta] of Object.entries(detailPageMeta)) {
    if (pathname.startsWith(prefix) && pathname.length > prefix.length) {
      return meta;
    }
  }

  return {
    title: "页面不存在",
    description: "该地址尚未注册，或对应的业务页面还未实现。",
  };
};

/** 当前路径命中的一级导航（用于高亮）。详情页回退到它的列表项。 */
export const findActiveItem = (pathname: string): NavigationItem | undefined => {
  /*
    精确匹配优先。

    这里**不能**再有 `/tasks` 特判：任务菜单现在是固定的
    `/tasks/cn` 与 `/tasks/intl` 两个真实项，按前缀精确命中即可。
    早先的写法是"只要以 /tasks/ 开头就返回 to === '/tasks' 的那一项"，
    但那个一级项已经不存在了 —— 于是返回 undefined，
    两个任务菜单都没有高亮。
  */
  const exact = matchItem(pathname);
  if (exact) {
    return exact;
  }

  // 详情页：/channels/wb-cn 应高亮「渠道管理」。
  const segments = pathname.split("/").filter(Boolean);
  if (segments.length > 1) {
    return navigationItems.find((item) => item.to === `/${segments[0]}`);
  }

  return undefined;
};
