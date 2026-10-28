import { ShieldX } from "lucide-react";
import { useEffect } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";

/**
 * 403：已登录但角色不够。
 *
 * 刻意不自动跳转：跳走会让人以为自己点错了链接。
 * 明确写出"这是权限问题"，并给一条能走的路（回到自己有权看的页面）。
 */
export const ForbiddenPage = () => {
  /*
    覆盖浏览器标签标题。
    页头（侧边栏右边那条）由 AppShell 按当前路由渲染，仍是"用户管理"——
    这没错：用户确实在访问那个地址。但标签标题若也写着"用户管理"，
    用户会以为自己进去了、只是内容空。改成明确的"没有访问权限"。
  */
  useEffect(() => {
    document.title = "没有访问权限 · relayAI";
  }, []);

  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
      <div className="flex size-12 items-center justify-center rounded-full bg-muted">
        <ShieldX className="size-6 text-muted-foreground" aria-hidden="true" />
      </div>
      <div className="flex flex-col gap-1">
        <h1 className="text-base font-medium">没有访问权限</h1>
        <p className="max-w-sm text-sm text-muted-foreground">
          这个页面只对管理员开放。如果你认为这是误判，请让管理员调整你的角色。
        </p>
      </div>
      <div className="flex items-center gap-2">
        <Button variant="outline" size="sm" asChild>
          <Link to="/dashboard">回到仪表盘</Link>
        </Button>
        <Button size="sm" asChild>
          <Link to="/account">我的账户</Link>
        </Button>
      </div>
    </div>
  );
};
