import { Menu, Wallet } from "lucide-react";
import { useState } from "react";
import { Link, Outlet, useLocation, useNavigate } from "react-router-dom";
import {
  Sidebar,
} from "@/components/common/AppShell/Sidebar";
import { UserMenu } from "@/components/common/AppShell/UserMenu";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet";
import { useBalanceRefresh } from "@/hooks/useBalanceRefresh";
import { usePageMeta } from "@/hooks/usePageMeta";
import { getPageMeta } from "@/routes/navigation";
import { useAuthStore } from "@/store/auth";

export type AppShellProps = {
};

export const AppShell = () => {
  const [isNavigationOpen, setNavigationOpen] = useState(false);
  const location = useLocation();
  const navigate = useNavigate();
  const page = getPageMeta(location.pathname);

  const user = useAuthStore((state) => state.user);
  const logout = useAuthStore((state) => state.logout);

  usePageMeta(page.title);
  // 顶栏余额每 30 秒刷新一次，并在页面重新可见时立即补一次。
  useBalanceRefresh();

  const sidebar = (onNavigate?: () => void) => (
    <Sidebar
      onNavigate={onNavigate}
      pathname={location.pathname}
      role={user?.role ?? "user"}
    />
  );

  const handleLogout = async () => {
    await logout();
    navigate("/login", { replace: true });
  };

  return (
    <div className="flex h-dvh overflow-hidden bg-background">
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:top-3 focus:left-3 focus:z-50 focus:rounded-md focus:border focus:bg-background focus:px-3 focus:py-2 focus:text-sm"
      >
        跳到主要内容
      </a>

      <aside className="hidden w-60 shrink-0 border-r border-sidebar-border md:block">
        {sidebar()}
      </aside>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b bg-background px-4 md:px-6">
          <Button
            variant="ghost"
            size="icon-sm"
            className="md:hidden"
            onClick={() => setNavigationOpen(true)}
            aria-label="打开导航菜单"
          >
            <Menu aria-hidden="true" />
          </Button>

          <div className="min-w-0">
            <h1 className="truncate text-sm leading-5 font-medium">
              {page.title}
            </h1>
            <p className="truncate text-xs leading-4 text-muted-foreground">
              {page.description}
            </p>
          </div>

          {/*
            余额常驻顶栏：它是用户最关心的数字，藏在用户中心里
            意味着每次都得点两下才能确认"还够不够用"。
            右边跟一个用户菜单（个人资料 / API 密钥 / 退出登录）。
          */}
          {user ? (
            <div className="ml-auto flex items-center gap-2">
              <Link
                to="/account"
                className="flex items-center gap-1.5 rounded-lg border bg-card px-2.5 py-1 text-xs transition-colors hover:bg-muted/60"
              >
                <Wallet className="size-3.5 text-muted-foreground" aria-hidden="true" />
                <span className="font-medium tabular-nums">
                  ¥{user.balance.toFixed(2)}
                </span>
              </Link>
              <UserMenu
                nickname={user.nickname}
                email={user.email}
                balance={user.balance}
                role={user.role}
                onLogout={() => void handleLogout()}
              />
            </div>
          ) : null}
        </header>

        <main
          id="main-content"
          tabIndex={-1}
          className="flex min-h-0 flex-1 flex-col overflow-x-hidden overflow-y-auto"
        >
          {/*
            内容区不再限制 `max-w-[1400px]`：仪表盘与统计页是宽表格 / 多列卡片，
            在 1920 的屏幕上留出两侧大片空白反而显得局促。
            改成"留一点边距、占满可用宽度"，由各页自己决定内部栅格列数。
            左右内边距给足（lg 以上 32px）：内容贴到侧边栏与窗口边缘时，
            卡片阴影会视觉上"切"在边界上，看起来像溢出。

            **这个包裹层必须是 `flex flex-1 flex-col` 且带上 `min-h-0`**：
            `main` 是滚动容器，页面根节点靠 `flex-1` 撑满剩余高度（表格要占满、
            分页要贴在底部）。中间少一层 flex 传递，`flex-1` 就没有可分配的高度，
            表格会缩成内容高度、下方留一大片空白。
          */}
          <div className="flex min-h-0 w-full min-w-0 flex-1 flex-col px-4 py-4 sm:px-6 md:px-8 md:py-6 lg:px-10">
            <Outlet />
          </div>
        </main>
      </div>

      <Sheet open={isNavigationOpen} onOpenChange={setNavigationOpen}>
        <SheetContent side="left" className="w-72 max-w-[85vw] p-0">
          <SheetTitle className="sr-only">主导航</SheetTitle>
          <SheetDescription className="sr-only">
            在管理台页面之间导航
          </SheetDescription>
          {sidebar(() => setNavigationOpen(false))}
        </SheetContent>
      </Sheet>
    </div>
  );
};
