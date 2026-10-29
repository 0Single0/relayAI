import { NavLink } from "react-router-dom";
import {
  findActiveItem,
  navigationGroups,
  type NavigationItem,
} from "@/routes/navigation";
import { cn } from "@/lib/utils";

export type SidebarTaskChannel = {
  id: string;
  name: string;
  pending: number;

};

export type SidebarProps = {
  onNavigate?: () => void;
  /** 任务组二级菜单：有运营能力的渠道。空数组时不渲染二级项。 */
  /** 当前路径，用于高亮二级项。 */
  pathname?: string;
  /** 当前用户角色；非 `admin` 会隐藏 `adminOnly` 的组与项。 */
  role?: string;
};

const itemClass = (isActive: boolean) =>
  cn(
    "group flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm transition-colors",
    "focus-visible:ring-3 focus-visible:ring-sidebar-ring/50 focus-visible:outline-none",
    isActive
      ? "bg-sidebar-accent font-medium text-sidebar-accent-foreground"
      : "text-sidebar-foreground hover:bg-sidebar-accent/60",
  );

const renderItem = (
  item: NavigationItem,
  isActive: boolean,
  onNavigate?: () => void,
) => {
  const Icon = item.icon;

  return (
    <NavLink
      to={item.to}
      end={item.end}
      onClick={onNavigate}
      className={itemClass(isActive)}
    >
      {({ isActive: linkActive }) => (
        <>
          <Icon
            className={cn(
              "size-4 shrink-0",
              linkActive ? "text-primary" : "text-muted-foreground",
            )}
            aria-hidden="true"
          />
          <span className="min-w-0 flex-1 truncate">{item.label}</span>
        </>
      )}
    </NavLink>
  );
};

export const Sidebar = ({
  onNavigate,
  pathname = "",
  role = "user",
}: SidebarProps) => {
  const activeItem = findActiveItem(pathname);
  const isAdmin = role === "admin";

  /**
   * 按角色过滤导航。
   *
   * 只是"别让用户看到进不去的门"：真正的拦截在路由层（`RequireAdmin`）
   * 与后端（`requireAdmin` → 403）。这里少一项，不影响安全性。
   */
  const groups = navigationGroups
    .filter((group) => isAdmin || !group.adminOnly)
    .map((group) => ({
      ...group,
      items: group.items.filter((item) => isAdmin || !item.adminOnly),
    }))
    .filter((group) => group.items.length > 0);

  return (
    <div className="flex h-full min-h-0 flex-col bg-sidebar text-sidebar-foreground">
      <div className="flex h-14 shrink-0 items-center gap-2.5 border-b border-sidebar-border px-4">
        {/*
          品牌标识用 `public/brand/logo.png`（用户提供的项目 logo），
          不用 lucide 图标：图标只是占位，logo 才是品牌资产。
          白丝带是图片里的负空间，所以这里不加任何底色/圆角容器。
        */}
        <img
          src="/brand/logo.png"
          alt=""
          aria-hidden="true"
          className="h-7 w-auto shrink-0 object-contain"
        />
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold tracking-tight">
            relayAI
          </p>
          <p className="truncate text-[11px] text-muted-foreground">
            反代管理台
          </p>
        </div>
      </div>

      <nav
        className="min-h-0 flex-1 overflow-y-auto px-3 py-3"
        aria-label="主导航"
      >
        {groups.map((group) => (
          <div key={group.label} className="mb-4 last:mb-0">
            <p className="px-2 pb-1.5 text-[11px] font-medium tracking-widest text-muted-foreground uppercase">
              {group.label}
            </p>
            <ul className="space-y-0.5">
              {group.items.map((item) => (
                <li key={item.to}>
                  {renderItem(item, activeItem?.to === item.to, onNavigate)}
                </li>
              ))}

            </ul>
          </div>
        ))}
      </nav>

      <div className="shrink-0 border-t border-sidebar-border p-3">
        <p className="text-[11px] leading-4 text-muted-foreground">
          开发中 · 数据来自本地网关
        </p>
      </div>
    </div>
  );
};
