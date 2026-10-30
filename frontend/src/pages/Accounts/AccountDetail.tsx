import { ArrowLeft, ListChecks, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  deleteAccount,
  getAccount,
  getAccountQuota,
  probeAccount,
  refreshCredential,
  updateAccount,
  type QuotaSnapshot,
} from "@/api/accounts";
import { listUsage } from "@/api/usage";
import type { Account, UsageLog } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { AccountStatusCell } from "@/pages/Accounts/components/AccountStatusCell";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const formatTime = (value: string | null) =>
  value ? new Date(value).toLocaleString("zh-CN") : "—";

export const AccountDetailPage = () => {
  const { id = "" } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const accountId = Number(id);

  const [account, setAccount] = useState<Account | null>(null);
  const [quota, setQuota] = useState<QuotaSnapshot | null>(null);
  const [usage, setUsage] = useState<UsageLog[]>([]);
  const [error, setError] = useState("");
  const [usageError, setUsageError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [actionMessage, setActionMessage] = useState("");
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    if (!Number.isFinite(accountId)) {
      setError("账号 id 无效");
      setIsLoading(false);
      return;
    }

    setIsLoading(true);
    try {
      const response = await getAccount(accountId);
      const next = response.data ?? null;
      setAccount(next);
      setError("");

      if (next) {
        const [quotaResponse, usageResponse] = await Promise.all([
          getAccountQuota(accountId).catch(() => null),
          listUsage({ pageSize: 50 }).catch(() => null),
        ]);
        setQuota(quotaResponse?.data ?? null);
        setUsage(
          (usageResponse?.data?.list ?? []).filter(
            (log) => log.accountUid === next.uid && log.channelId === next.channelId,
          ),
        );
        setUsageError(usageResponse ? "" : "最近请求加载失败");
      }
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, [accountId]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const runAction = useCallback(
    async (key: string, action: () => Promise<string>) => {
      setBusy(key);
      setActionMessage("");
      try {
        const message = await action();
        setActionMessage(message);
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        setActionMessage(`操作失败：${getRequestErrorMessage(requestError)}`);
      } finally {
        setBusy("");
      }
    },
    [],
  );

  const usageColumns: Array<Column<UsageLog>> = [
    {
      key: "ts",
      header: "时间",
      width: "160px",
      render: (log) => (
        <span className="text-xs text-muted-foreground tabular-nums">
          {new Date(log.ts).toLocaleString("zh-CN")}
        </span>
      ),
    },
    {
      key: "model",
      header: "模型",
      render: (log) => <span className="font-mono text-xs">{log.model}</span>,
    },
    {
      key: "tokens",
      header: "Token",
      width: "110px",
      align: "right",
      render: (log) => (
        <span className="tabular-nums text-xs">
          {log.promptTokens}/{log.completionTokens}
        </span>
      ),
    },
    {
      key: "ttft",
      header: "首字",
      width: "80px",
      align: "right",
      hideOnMobile: true,
      render: (log) => (
        <span className="tabular-nums text-xs text-muted-foreground">
          {log.ttftMs === null ? "—" : `${log.ttftMs}ms`}
        </span>
      ),
    },
    {
      key: "total",
      header: "耗时",
      width: "80px",
      align: "right",
      hideOnMobile: true,
      render: (log) => (
        <span className="tabular-nums text-xs text-muted-foreground">
          {log.totalMs}ms
        </span>
      ),
    },
    {
      key: "status",
      header: "状态",
      width: "100px",
      render: (log) => (
        <StatusBadge
          tone={
            log.status === "completed"
              ? "success"
              : log.status === "rejected"
                ? "neutral"
                : "danger"
          }
          label={log.status}
        />
      ),
    },
  ];

  if (isLoading && !account) {
    return (
      <MyBox isLoading text="正在加载账号..." className="min-h-40">
        <div />
      </MyBox>
    );
  }

  if (error || !account) {
    return (
      <div className="flex flex-col gap-3">
        <Button variant="ghost" size="sm" asChild className="self-start">
          <Link to="/accounts">
            <ArrowLeft aria-hidden="true" />
            返回账号列表
          </Link>
        </Button>
        <ErrorState
          message={error || "账号不存在"}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="ghost" size="sm" asChild>
          <Link to="/accounts">
            <ArrowLeft aria-hidden="true" />
            返回
          </Link>
        </Button>
        <span className="font-mono text-xs text-muted-foreground">
          {account.channelId}/{account.uid}
        </span>
        <AccountStatusCell account={account} />
        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== ""}
            onClick={() =>
              void runAction("probe", async () => {
                const response = await probeAccount(accountId);
                const result = response.data;
                return result?.ok
                  ? `探活成功 · ${result.latencyMs}ms`
                  : `探活失败：${result?.error ?? "未知原因"}`;
              })
            }
          >
            <RefreshCw
              className={cn(
                busy === "probe" && "animate-spin motion-reduce:animate-none",
              )}
              aria-hidden="true"
            />
            探活
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== ""}
            onClick={() =>
              void runAction("refresh", async () => {
                const response = await refreshCredential(accountId);
                return response.data?.message ?? "凭据已刷新";
              })
            }
          >
            刷新凭据
          </Button>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => setDeleteOpen(true)}
          >
            <Trash2 aria-hidden="true" />
            删除
          </Button>
        </div>
      </div>

      {actionMessage ? (
        <p className="rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          {actionMessage}
        </p>
      ) : null}

      <div className="grid min-w-0 grid-cols-1 gap-3 lg:grid-cols-2">
        <PageSection title="凭据" description="有效期与最近刷新">
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 text-xs">
            <dt className="text-muted-foreground">渠道</dt>
            <dd className="min-w-0 truncate">{account.channelName || account.channelId}</dd>
            <dt className="text-muted-foreground">类型</dt>
            <dd>{account.adapter}</dd>
            <dt className="text-muted-foreground">凭据过期</dt>
            <dd className="tabular-nums">
              {formatTime(account.runtime?.credentialExpiresAt ?? null)}
            </dd>
            <dt className="text-muted-foreground">最近成功</dt>
            <dd className="tabular-nums">{formatTime(account.runtime?.lastOkAt ?? null)}</dd>
            <dt className="text-muted-foreground">最近失败</dt>
            <dd className="min-w-0 break-words text-destructive">
              {account.runtime?.lastError ?? "—"}
            </dd>
          </dl>

          <div className="mt-3 flex items-center gap-2 border-t pt-3">
            <Switch
              checked={account.enabled}
              aria-label="启用账号"
              onCheckedChange={(checked) =>
                void runAction("toggle", async () => {
                  await updateAccount(accountId, { enabled: checked });
                  return checked ? "账号已启用" : "账号已停用";
                })
              }
              disabled={busy !== ""}
            />
            <span className="text-xs text-muted-foreground">
              {account.enabled ? "已启用" : "已停用"}
            </span>
          </div>
        </PageSection>

        <PageSection title="额度" description="来自上游的额度快照">
          {quota === null ? (
            <EmptyState
              title="该账号没有额度快照"
              description="渠道未实现额度查询，或尚未抓取过。"
            />
          ) : (
            <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 text-xs">
              <dt className="text-muted-foreground">{quota.label || "剩余"}</dt>
              <dd className="tabular-nums">
                {quota.remaining ?? "—"} {quota.unit}
                {quota.total !== null ? (
                  <span className="ml-1 text-muted-foreground">
                    / {quota.total} {quota.unit}
                  </span>
                ) : null}
              </dd>
              <dt className="text-muted-foreground">抓取时间</dt>
              <dd className="tabular-nums">{formatTime(quota.fetchedAt)}</dd>
              {quota.details.map(([key, value]) => (
                <div key={key} className="contents">
                  <dt className="min-w-0 truncate text-muted-foreground">{key}</dt>
                  <dd className="min-w-0 break-words">{value}</dd>
                </div>
              ))}
            </dl>
          )}
        </PageSection>
      </div>

      <PageSection
        title="最近请求"
        description="该账号最近 50 条请求记录"
        bodyClassName="p-0"
      >
        {usageError ? (
          <div className="p-4">
            <ErrorState message={usageError} />
          </div>
        ) : (
          <DataTable
            columns={usageColumns}
            rows={usage}
            rowKey={(log) => String(log.id)}
            empty={
              <EmptyState
                icon={ListChecks}
                title="还没有请求记录"
                description="该账号被调度使用后，这里会出现请求明细。"
              />
            }
          />
        )}
      </PageSection>

      <Button variant="outline" size="sm" asChild className="self-start">
        {/*
          任务页按**版本**取号，不按分组 —— 未分配分组的号也能点进来。
          之前直接用 `account.channelId`，待分配的号会拼出一个空路径 `/tasks/`。
        */}
        <Link
          to={`/tasks/${account.options?.realm === "intl" ? "intl" : "cn"}?account=${encodeURIComponent(account.uid)}`}
        >
          <ListChecks aria-hidden="true" />
          查看该账号的任务
        </Link>
      </Button>

      <Dialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              将删除 {account.nickname || account.uid} 的凭据。历史请求记录会保留。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setDeleteOpen(false)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={() =>
                void runAction("delete", async () => {
                  await deleteAccount(accountId);
                  navigate("/accounts");
                  return "账号已删除";
                })
              }
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};
