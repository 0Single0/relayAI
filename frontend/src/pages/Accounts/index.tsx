import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Ban,
  CheckCircle2,
  PencilLine,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  UserRound,
} from "lucide-react";
import { deleteAccount, listAccounts, updateAccount } from "@/api/accounts";
import { listChannels } from "@/api/channels";
import { listProxies, setAccountProxy, type Proxy } from "@/api/proxies";
import type { Account, Channel } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Pagination } from "@/components/common/Pagination";
import { QuotaBar } from "@/components/common/QuotaBar";
import { RowActions } from "@/components/common/RowActions";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { AccountStatusCell } from "@/pages/Accounts/components/AccountStatusCell";
import { CreateAccountDrawer } from "@/pages/Accounts/components/CreateAccountDrawer";
import { toast } from "@/components/common/Toast/store";
import { platformLabel } from "@/pages/Channels/shared";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const ALL = "__all__";

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

/**
 * 账号列表。
 *
 * **额度直接进表格**：账号的积分/余额是这个页最需要一眼看到的信息，
 * 藏进二级详情页等于每次都要多点一次。额度来自网关内存快照
 * （巡检每 5 分钟刷一次），所以列出来不增加任何上游请求。
 *
 * 刻意**没有**的东西：
 *   · 「最近失败」列 —— 排障信息放详情；列表里绝大多数行都是「—」，白占地方。
 *   · 「最近请求」列 —— 同上，而且它和「最近使用」重复。
 *   · 二级详情页入口 —— 用户明确要求不要，所有操作都在行内完成。
 */
export const AccountsPage = () => {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [channelFilter, setChannelFilter] = useState(ALL);
  const [keyword, setKeyword] = useState("");
  const [reloadKey, setReloadKey] = useState(0);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const [createOpen, setCreateOpen] = useState(false);
  const [editing, setEditing] = useState<Account | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Account | null>(null);
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);
  const [busy, setBusy] = useState("");

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const [accountResponse, channelResponse, proxyResponse] = await Promise.all([
        listAccounts({ pageSize: 200 }),
        listChannels().catch(() => null),
        listProxies().catch(() => null),
      ]);
      setAccounts(accountResponse.data?.list ?? []);
      setChannels(channelResponse?.data ?? []);
      setProxies(proxyResponse?.data ?? []);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    return accounts.filter((account) => {
      if (channelFilter !== ALL && account.channelId !== channelFilter) {
        return false;
      }
      if (!trimmed) {
        return true;
      }
      return (
        account.uid.toLowerCase().includes(trimmed) ||
        account.nickname.toLowerCase().includes(trimmed) ||
        account.channelId.toLowerCase().includes(trimmed) ||
        account.platform.toLowerCase().includes(trimmed)
      );
    });
  }, [accounts, channelFilter, keyword]);

  useEffect(() => {
    setPage(1);
  }, [channelFilter, keyword]);

  const paged = useMemo(
    () => filtered.slice((page - 1) * pageSize, page * pageSize),
    [filtered, page, pageSize],
  );

  /**
   * 刷新凭据：**只有会过期的凭据才能刷新**。
   *
   * 静态凭据（自定义供应商的 API Key）永远不会过期，没有"刷新"这回事；
   * 给它一个可点的刷新按钮，点下去只会得到一个 501/400，
   * 用户会以为是系统坏了。所以这里直接把它置灰并说明原因。
   */
  const canRefresh = (account: Account) =>
    account.runtime !== null && !account.runtime.staticCredential;

  const refresh = useCallback(
    async (account: Account) => {
      if (!canRefresh(account)) {
        return;
      }
      setBusy(`refresh-${account.id}`);
      toast.success("");
      try {
        await updateAccount(account.id, {});
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        setError(getRequestErrorMessage(requestError));
      } finally {
        setBusy("");
      }
    },
    [],
  );

  const toggleEnabled = useCallback(async (account: Account) => {
    try {
      await updateAccount(account.id, { enabled: !account.enabled });
      setReloadKey((value) => value + 1);
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    }
  }, []);

  const deleteMany = useCallback(
    async (ids: string[]) => {
      setBusy("delete");
      setError("");
      try {
        for (const key of ids) {
          const account = accounts.find((item) => String(item.id) === key);
          if (account) {
            await deleteAccount(account.id);
          }
        }
        setSelected(new Set());
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        setError(getRequestErrorMessage(requestError));
      } finally {
        setBusy("");
      }
    },
    [accounts],
  );

  const columns: Array<Column<Account>> = useMemo(
    () => [
      {
        key: "account",
        header: "账号",
        width: "20%",
        // 唯一左对齐的列：昵称 + uid 是长文本，居中会让两行长度不一、左右都参差。
        align: "left",
        render: (account) => (
          <div className="min-w-0">
            <p className="truncate font-medium">
              {account.nickname || account.uid}
            </p>
            <p className="truncate font-mono text-[11px] text-muted-foreground">
              {account.uid}
            </p>
          </div>
        ),
      },
      {
        key: "platform",
        header: "平台",
        width: "112px",
        render: (account) => (
          <StatusBadge tone="neutral" label={platformLabel(account.platform)} dot={false} />
        ),
      },
      {
        key: "channel",
        header: "分组",
        width: "124px",
        render: (account) => (
          <span className="block truncate text-xs text-muted-foreground">
            {account.channelName || account.channelId}
          </span>
        ),
      },
      {
        key: "quota",
        header: "额度",
        width: "152px",
        // 额度是这个页最该一眼看到的东西，所以放表格里而不是详情页。
        // 用进度条而不是纯数字：剩余/总量一眼能看出"还能撑多久"，
        // 纯数字（847）没有参照，读者不知道这是多是少。
        render: (account) => {
          const quota = account.runtime?.quota;
          if (!quota) {
            return <span className="text-xs text-muted-foreground">—</span>;
          }
          return (
            <QuotaBar
              remaining={quota.remaining}
              total={quota.total}
              label={quota.label}
              unit={quota.unit}
            />
          );
        },
      },
      {
        key: "capacity",
        header: "在飞",
        width: "80px",
        align: "center",
        render: (account) => <CapacityCell account={account} />,
      },
      {
        key: "today",
        header: "今日",
        width: "116px",
        align: "center",
        hideOnMobile: true,
        render: (account) => {
          const usage = account.usage;
          if (!usage) {
            return <span className="text-xs text-muted-foreground">—</span>;
          }
          return (
            <div className="flex flex-col items-end leading-4">
              <span className="tabular-nums">
                {usage.todayRequests.toLocaleString("zh-CN")} 次
              </span>
              <span className="text-[11px] tabular-nums text-muted-foreground">
                {compactNumber(usage.todayTokens)} token
              </span>
            </div>
          );
        },
      },
      {
        key: "proxy",
        header: "代理",
        width: "120px",
        hideOnMobile: true,
        // 出口 IP 是排障时最需要的上下文之一：上游 429/403 时，
        // 第一件事就是确认"这个号从哪个 IP 出去的"。
        render: (account) => (
          <span className="block truncate text-xs text-muted-foreground">
            {account.proxyName ?? "直连"}
          </span>
        ),
      },
      {
        key: "status",
        header: "状态",
        width: "112px",
        render: (account) => <AccountStatusCell account={account} />,
      },
      {
        key: "actions",
        header: "操作",
        width: "220px",
        align: "center",
        render: (account) => {
          const refreshable = canRefresh(account);
          return (
            <RowActions
              actions={[
                {
                  label: refreshable ? "刷新凭据" : "静态凭据无需刷新",
                id: "refresh",
                  icon: RefreshCw,
                  tone: "primary",
                  disabled: !refreshable || busy === `refresh-${account.id}`,
                  onClick: () => void refresh(account),
                },
                {
                  label: account.enabled ? "禁用" : "启用",
                id: "toggle",
                  icon: account.enabled ? Ban : CheckCircle2,
                  tone: "warning",
                  onClick: () => void toggleEnabled(account),
                },
                {
                  label: "编辑",
                id: "edit",
                  icon: PencilLine,
                  onClick: () => setEditing(account),
                },
                {
                  label: "删除",
                id: "delete",
                  icon: Trash2,
                  tone: "danger",
                  onClick: () => setDeleteTarget(account),
                },
              ]}
            />
          );
        },
      },
    ],
    [busy, refresh, toggleEnabled],
  );

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-40 shrink-0">
          <Select value={channelFilter} onValueChange={setChannelFilter}>
            <SelectTrigger aria-label="按分组筛选">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>全部分组</SelectItem>
              {channels.map((channel) => (
                <SelectItem key={channel.id} value={channel.id}>
                  {channel.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="relative min-w-0 flex-1 sm:max-w-xs">
          <Search
            className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <input
            type="search"
            value={keyword}
            onChange={(event) => setKeyword(event.target.value)}
            placeholder="搜索昵称、uid、平台"
            aria-label="搜索账号"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

        {selected.size > 0 ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => setBulkDeleteOpen(true)}
            disabled={busy === "delete"}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            <Trash2 aria-hidden="true" />
            删除所选（{selected.size}）
          </Button>
        ) : null}

        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setReloadKey((value) => value + 1)}
            disabled={isLoading}
          >
            <RefreshCw
              className={cn(isLoading && "animate-spin motion-reduce:animate-none")}
              aria-hidden="true"
            />
            刷新
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus aria-hidden="true" />
            新建账号
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}



      <MyBox
        isLoading={isLoading && accounts.length === 0}
        text="正在加载账号..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={paged}
          rowKey={(account) => String(account.id)}
          selection={{
            selected,
            onChange: setSelected,
            label: (account) => `选择账号 ${account.nickname || account.uid}`,
          }}
          empty={
            accounts.length === 0 ? (
              <EmptyState
                icon={UserRound}
                title="还没有任何账号"
                description="账号是一份上游凭据。选一个平台（WorkBuddy / Trae / 自定义），填好凭据就能开始转发。"
                action={
                  <Button size="sm" onClick={() => setCreateOpen(true)}>
                    <Plus aria-hidden="true" />
                    新建账号
                  </Button>
                }
              />
            ) : (
              <EmptyState
                icon={Search}
                title="没有匹配的账号"
                description="换个关键字，或把渠道筛选恢复为「全部分组」。"
                action={
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      setKeyword("");
                      setChannelFilter(ALL);
                    }}
                  >
                    重置筛选
                  </Button>
                }
              />
            )
          }
        />
      </MyBox>

      <Pagination
        page={page}
        pageSize={pageSize}
        total={filtered.length}
        onPageChange={setPage}
        onPageSizeChange={(size) => {
          setPageSize(size);
          setPage(1);
        }}
        disabled={isLoading}
      />

      <CreateAccountDrawer
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={() => setReloadKey((value) => value + 1)}
      />

      <EditAccountDialog
        account={editing}
        channels={channels}
        proxies={proxies}
        onClose={() => setEditing(null)}
        onDone={() => {
          setEditing(null);
          setReloadKey((value) => value + 1);
        }}
      />

      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(next) => !next && setDeleteTarget(null)}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>删除账号</DialogTitle>
            <DialogDescription>
              删除「{deleteTarget?.nickname || deleteTarget?.uid}」后，
              网关不再用它转发请求。历史用量记录保留。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setDeleteTarget(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                const target = deleteTarget;
                setDeleteTarget(null);
                if (target) {
                  void deleteMany([String(target.id)]);
                }
              }}
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={bulkDeleteOpen} onOpenChange={setBulkDeleteOpen}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>批量删除</DialogTitle>
            <DialogDescription>
              将删除选中的 {selected.size} 个账号，无法恢复。历史用量记录保留。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setBulkDeleteOpen(false)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                const ids = [...selected];
                setBulkDeleteOpen(false);
                void deleteMany(ids);
              }}
            >
              删除 {selected.size} 个
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

/**
 * 在飞：`inflight / maxInflight`。
 *
 * 上限只能取 `runtime.maxInflight`（网关已把库里的 NULL 解析成渠道默认值）。
 * 用 `account.maxInflight` 会把 NULL 渲染成 `∞`，那是在说"不限并发"——
 * 而网关实际按 3 在限流，两者相反。
 */
const CapacityCell = ({ account }: { account: Account }) => {
  const runtime = account.runtime;
  if (!runtime) {
    return <span className="text-xs text-muted-foreground">—</span>;
  }

  const max = runtime.maxInflight;
  const current = runtime.inflight;

  if (max <= 0) {
    return <span className="tabular-nums text-muted-foreground">{current}</span>;
  }

  const tone: StatusTone = current >= max ? "danger" : current > 0 ? "warning" : "neutral";

  return <StatusBadge tone={tone} label={`${current}/${max}`} dot={false} />;
};

/** 「直连」在下拉里的哨兵值（不能用空串：Radix Select 不接受空值项）。 */
const DIRECT = "__direct__";

/** 编辑账号：昵称 / 启用 / 并发上限 / 出站代理。 */
const EditAccountDialog = ({
  account,
  channels,
  proxies,
  onClose,
  onDone,
}: {
  account: Account | null;
  channels: Channel[];
  proxies: Proxy[];
  onClose: () => void;
  onDone: () => void;
}) => {
  const [nickname, setNickname] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [maxInflight, setMaxInflight] = useState("");
  const [proxyId, setProxyId] = useState<number | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (account) {
      setNickname(account.nickname);
      setEnabled(account.enabled);
      setMaxInflight(account.maxInflight === null ? "" : String(account.maxInflight));
      setProxyId(account.proxyId);
      setError("");
    }
  }, [account]);

  const submit = async () => {
    if (!account) {
      return;
    }
    setPending(true);
    setError("");
    try {
      await updateAccount(account.id, {
        nickname: nickname.trim(),
        enabled,
        maxInflight: maxInflight.trim() ? Number(maxInflight) : null,
      });
      // 代理走独立接口（它不在账号 PATCH 的字段里）：
      // 变更后网关要重载该账号的出站客户端，所以单独一次调用。
      if (proxyId !== account.proxyId) {
        await setAccountProxy(account.id, proxyId);
      }
      onDone();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={account !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>编辑账号</DialogTitle>
          <DialogDescription>
            {account
              ? `${platformLabel(account.platform)} · ${
                  channels.find((c) => c.id === account.channelId)?.name ?? account.channelId
                }`
              : ""}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">昵称</label>
            <input
              value={nickname}
              onChange={(event) => setNickname(event.target.value)}
              className={inputClass}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">状态</label>
            <Select
              value={enabled ? "on" : "off"}
              onValueChange={(next) => setEnabled(next === "on")}
            >
              <SelectTrigger aria-label="状态">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="on">启用</SelectItem>
                <SelectItem value="off">停用（不参与调度）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">并发上限</label>
            <input
              value={maxInflight}
              onChange={(event) => setMaxInflight(event.target.value)}
              inputMode="numeric"
              placeholder="留空 = 用渠道默认"
              className={cn(inputClass, "tabular-nums")}
            />
          </div>

          {/*
            出站代理：每个账号单独选。
            这是"把出口 IP 摊开"的关键 —— 上游按 IP 限流，
            所有号共用一个 IP 时几十个号会叠在一起被判异常。
          */}
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">出站代理</label>
            <Select
              value={proxyId === null ? DIRECT : String(proxyId)}
              onValueChange={(next) =>
                setProxyId(next === DIRECT ? null : Number(next))
              }
            >
              <SelectTrigger aria-label="出站代理">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={DIRECT}>直连（用服务器出口 IP）</SelectItem>
                {proxies.map((proxy) => (
                  <SelectItem key={proxy.id} value={String(proxy.id)}>
                    {proxy.name} · {proxy.protocol}://{proxy.host}:{proxy.port}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {proxies.length === 0 ? (
              <p className="text-[11px] text-muted-foreground">
                还没有代理。去「系统 → 代理」建一个，再回来绑定。
              </p>
            ) : null}
          </div>

          {error ? <ErrorState title="保存失败" message={error} /> : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
            取消
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending}>
            {pending ? "保存中..." : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
