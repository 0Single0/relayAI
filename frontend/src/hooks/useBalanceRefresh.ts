import { useEffect } from "react";
import { useAuthStore } from "@/store/auth";

/**
 * 余额自动刷新。
 *
 * 两个触发点，缺一不可：
 *   1. **定时轮询**（30 秒）：用户可能就停在这一页看图表，
 *      期间请求在跑、余额在掉，不刷新就会看着一个过期的数字做判断；
 *   2. **页面重新可见**（切标签页回来 / 从后台恢复）：浏览器会节流后台标签
 *      的定时器，30 秒的间隔在后台可能变成几分钟。回到页面时立刻补一次，
 *      比等下一个 tick 更符合"我回来了，现在多少钱"的预期。
 *
 * 为什么不做成全局 store 里的定时器：
 *   它是**纯展示**需求（顶栏那个数字）。放在 AppShell 里，
 *   组件卸载就自动停，不必操心登录/登出的启停逻辑。
 *
 * 为什么静默失败：余额刷新失败不该弹 toast —— 用户没做任何操作，
 * 弹一个"刷新失败"只会打断他。真正的鉴权失效由 request 层统一处理。
 */
const BALANCE_POLL_MS = 30_000;

export const useBalanceRefresh = () => {
  const user = useAuthStore((state) => state.user);
  const refresh = useAuthStore((state) => state.refresh);

  useEffect(() => {
    // 未登录时不轮询：没有令牌，每个 tick 都会 401。
    if (!user) {
      return;
    }

    const tick = () => {
      // 后台标签不必刷新：既省请求，也避免"看不见的地方在动"。
      if (document.visibilityState === "visible") {
        void refresh().catch(() => {
          // 静默：见上方注释。
        });
      }
    };

    const timer = window.setInterval(tick, BALANCE_POLL_MS);

    // 从后台回到前台时立即补一次，不等下一个 30 秒。
    const onVisible = () => {
      if (document.visibilityState === "visible") {
        void refresh().catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [user, refresh]);
};
