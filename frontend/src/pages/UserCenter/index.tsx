import { useCallback, useEffect, useState } from "react";
import { Coins, Loader2, Receipt, Ticket } from "lucide-react";
import { listTransactions, redeem, type BalanceTransaction } from "@/api/auth";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { useAuthStore } from "@/store/auth";
import { cn } from "@/lib/utils";
import { toast } from "@/components/common/Toast/store";
import { getRequestErrorMessage } from "@/utils/request";

const formatMoney = (value: number) => `¥${value.toFixed(2)}`;

const kindLabel = (kind: BalanceTransaction["kind"]) => {
  switch (kind) {
    case "redeem":
      return "兑换充值";
    case "usage":
      return "调用扣费";
    default:
      return "调整";
  }
};

/**
 * 用户中心：余额、兑换码、流水。
 *
 * 余额放在最上面：用户来这一页的第一件事几乎总是"我还有多少钱"。
 * 兑换入口与余额同屏 —— 充值是这一页唯一的高频动作。
 */
export const UserCenterPage = () => {
  const user = useAuthStore((state) => state.user);
  const refresh = useAuthStore((state) => state.refresh);

  const [code, setCode] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  const [transactions, setTransactions] = useState<BalanceTransaction[]>([]);
  const [listError, setListError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listTransactions({ pageSize: 50 });
      setTransactions(response.data?.list ?? []);
      setListError("");
    } catch (requestError) {
      setListError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const submit = async () => {
    if (!code.trim()) {
      return;
    }
    setPending(true);
    setError("");
    try {
      const response = await redeem(code.trim());
      toast.success("充值成功", `+${formatMoney(response.data?.amount ?? 0)}`);
      setCode("");
      await refresh();
      setReloadKey((value) => value + 1);
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  const inputClass = cn(
    "h-8 w-full rounded-md border border-input bg-background px-2.5 font-mono text-sm uppercase",
    "placeholder:text-muted-foreground placeholder:normal-case",
    "hover:border-border-strong",
    "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
  );

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <div className="rounded-xl border bg-card p-4">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Coins className="size-3.5" aria-hidden="true" />
            可用余额
          </div>
          <p className="mt-1.5 text-2xl font-semibold tabular-nums">
            {formatMoney(user?.balance ?? 0)}
          </p>
        </div>
        <div className="rounded-xl border bg-card p-4">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Ticket className="size-3.5" aria-hidden="true" />
            累计充值
          </div>
          <p className="mt-1.5 text-2xl font-semibold tabular-nums">
            {formatMoney(user?.totalRecharged ?? 0)}
          </p>
        </div>
        <div className="rounded-xl border bg-card p-4">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Receipt className="size-3.5" aria-hidden="true" />
            账号
          </div>
          <p className="mt-1.5 truncate text-sm font-medium">{user?.nickname}</p>
          <p className="truncate font-mono text-[11px] text-muted-foreground">
            {user?.email}
          </p>
        </div>
      </div>

      <PageSection title="兑换码充值" description="输入兑换码即可充值到余额">
        <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
          <input
            value={code}
            onChange={(event) => setCode(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void submit();
              }
            }}
            placeholder="XXXX-XXXX-XXXX-XXXX"
            aria-label="兑换码"
            className={cn(inputClass, "sm:max-w-xs")}
          />
          <Button size="sm" onClick={() => void submit()} disabled={pending || !code.trim()}>
            {pending ? (
              <Loader2 className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
            ) : null}
            立即兑换
          </Button>

        </div>
        {error ? <ErrorState title="兑换失败" message={error} /> : null}
      </PageSection>

      <PageSection title="余额流水" description="每一笔充值与扣费" bodyClassName="p-0">
        <MyBox isLoading={isLoading && transactions.length === 0} text="正在读取流水...">
          {listError ? (
            <ErrorState
              message={listError}
              onRetry={() => setReloadKey((value) => value + 1)}
              retrying={isLoading}
            />
          ) : transactions.length === 0 ? (
            <EmptyState
              icon={Receipt}
              title="还没有流水"
              description="用兑换码充值后，这里会出现记录。"
            />
          ) : (
            <ul className="divide-y">
              {transactions.map((item) => (
                <li
                  key={item.id}
                  className="flex flex-wrap items-center gap-2 px-4 py-2.5 text-sm"
                >
                  <StatusBadge
                    tone={item.kind === "redeem" ? "success" : "neutral"}
                    label={kindLabel(item.kind)}
                    dot={false}
                  />
                  <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                    {item.note || "—"}
                  </span>
                  <span
                    className={cn(
                      "w-24 shrink-0 text-right tabular-nums",
                      item.amount >= 0 ? "text-success" : "text-foreground",
                    )}
                  >
                    {item.amount >= 0 ? "+" : ""}
                    {formatMoney(item.amount)}
                  </span>
                  <span className="w-24 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
                    {formatMoney(item.balanceAfter)}
                  </span>
                  <span className="w-36 shrink-0 text-right text-[11px] tabular-nums text-muted-foreground">
                    {new Date(item.createdAt).toLocaleString("zh-CN")}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </MyBox>
      </PageSection>
    </div>
  );
};
