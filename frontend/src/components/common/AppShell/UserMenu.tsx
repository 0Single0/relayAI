import { useEffect, useRef, useState } from "react";
import { ChevronDown, KeyRound, LogOut, UserRound, Wallet } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/lib/utils";

/**
 * 页头用户菜单（对齐 sub2api 的形态）。
 *
 * 结构：`[余额] [昵称 ▾]` → 点开是
 *   标识（昵称 + 邮箱）
 *   个人资料 / API 密钥
 *   退出登录
 *
 * 为什么把「退出登录」从独立的图标按钮收进菜单：
 *   顶栏常驻控件越少越好。退出是低频破坏性操作，
 *   放在图标按钮上和「余额」并排，容易误点。
 *
 * 为什么自己写而不是用 Radix DropdownMenu：
 *   项目目前只装了 dialog/select/sheet 这几个原语，
 *   为这一个菜单引一个包不划算。这里实现的是最小可访问集合：
 *   点外部关闭、Esc 关闭、`aria-expanded` / `aria-haspopup`、焦点回到按钮。
 */
export const UserMenu = ({
  nickname,
  email,
  balance,
  role,
  onLogout,
}: {
  nickname: string;
  email: string;
  balance: number;
  role: "admin" | "user";
  onLogout: () => void;
}) => {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // 点外部 / Esc 关闭。两个都要有：只做外部点击时键盘用户无法收起菜单。
  useEffect(() => {
    if (!open) {
      return;
    }
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        // 焦点回到触发按钮，否则键盘用户会掉到 body 上。
        triggerRef.current?.focus();
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const display = nickname || email;
  // 头像取首字符：没有用户上传的头像，字母块比灰色空圈更有识别度。
  const initial = display.trim().slice(0, 1).toUpperCase() || "?";

  return (
    <div className="relative" ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="用户菜单"
        className={cn(
          "flex cursor-pointer items-center gap-2 rounded-lg border bg-card px-2 py-1 transition-colors",
          "hover:bg-muted/60 focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
          open && "bg-muted/60",
        )}
      >
        <span
          className="flex size-6 shrink-0 items-center justify-center rounded-full bg-primary text-[11px] font-medium text-primary-foreground"
          aria-hidden="true"
        >
          {initial}
        </span>
        <span className="hidden max-w-[10rem] truncate text-xs font-medium sm:block">
          {display}
        </span>
        <ChevronDown
          className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")}
          aria-hidden="true"
        />
      </button>

      {open ? (
        <div
          role="menu"
          aria-label="用户菜单"
          className="absolute right-0 z-50 mt-1.5 w-64 overflow-hidden rounded-lg border bg-popover shadow-lg"
        >
          {/* 标识区：昵称 + 邮箱 + 角色，回答"我现在是谁"。 */}
          <div className="flex flex-col gap-0.5 border-b px-3 py-2.5">
            <span className="truncate text-sm font-medium">{display}</span>
            <span className="truncate text-xs text-muted-foreground">{email}</span>
            <span className="mt-1 flex items-center gap-1 text-[11px] text-muted-foreground">
              <Wallet className="size-3" aria-hidden="true" />
              余额 ¥{balance.toFixed(2)}
              {/*
                角色用 `admin` 而不是中文"管理员"：昵称本身可能就叫"管理员"，
                两个中文词并排会看成重复渲染。
              */}
              {role === "admin" ? (
                <span
                  className="ml-auto rounded bg-primary/10 px-1.5 py-0.5 font-mono text-[10px] text-primary"
                  title="管理员：可管理分组、账号与用户"
                >
                  admin
                </span>
              ) : null}
            </span>
          </div>

          <div className="p-1">
            <Link
              to="/account"
              role="menuitem"
              onClick={() => setOpen(false)}
              className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-muted focus-visible:bg-muted focus-visible:outline-none"
            >
              <UserRound className="size-4 text-muted-foreground" aria-hidden="true" />
              个人资料
            </Link>
            <Link
              to="/keys"
              role="menuitem"
              onClick={() => setOpen(false)}
              className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-muted focus-visible:bg-muted focus-visible:outline-none"
            >
              <KeyRound className="size-4 text-muted-foreground" aria-hidden="true" />
              API 密钥
            </Link>
          </div>

          <div className="border-t p-1">
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onLogout();
              }}
              className="flex w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm text-destructive transition-colors hover:bg-destructive/10 focus-visible:bg-destructive/10 focus-visible:outline-none"
            >
              <LogOut className="size-4" aria-hidden="true" />
              退出登录
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
};
