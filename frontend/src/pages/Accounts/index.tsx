import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Ban,
  CheckCircle2,
  Coins,
  FlaskConical,
  PencilLine,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  UserRound,
} from "lucide-react";
import {
  listAccounts,
  refreshAccountQuotas,
  resetAccountCooldowns,
  deleteAccount,
  updateAccount,
} from "@/api/accounts";
import { listCatalogModels, type CatalogModel } from "@/api/catalog";
import { listChannels } from "@/api/channels";
import { listProxies, type Proxy } from "@/api/proxies";
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
import { ModelListButton, ModelListDialog } from "@/pages/Accounts/components/ModelListDialog";
import { ProbeDialog } from "@/pages/Accounts/components/ProbeDialog";
import { CreateAccountDrawer } from "@/pages/Accounts/components/CreateAccountDrawer";
import { EditAccountDialog } from "@/pages/Accounts/components/EditAccountDialog";
import { toast } from "@/components/common/Toast/store";
import { accountMatchesChannel, accountPlatformLabel } from "@/pages/Channels/shared";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const ALL = "__all__";
/** 筛出"还没分配分组"的号。与真实分组 id 不可能重名（分组 id 不以双下划线开头）。 */
const UNASSIGNED = "__unassigned__";

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
  /** 模型目录：给模型列算"这个账号能用哪些模型"。 */
  const [catalogModels, setCatalogModels] = useState<CatalogModel[]>([]);
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
  /** 正在测试凭据的账号；非空时打开测试弹窗。 */
  const [probing, setProbing] = useState<Account | null>(null);
  /** 查看模型清单的账号；非空时打开模型弹窗。 */
  const [modelTarget, setModelTarget] = useState<Account | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Account | null>(null);
  const [bulkDeleteOpen, setBulkDeleteOpen] = useState(false);
  const [busy, setBusy] = useState("");

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const [accountResponse, channelResponse, proxyResponse, catalogResponse] = await Promise.all([
        listAccounts({ pageSize: 200 }),
        listChannels().catch(() => null),
        listProxies().catch(() => null),
        // 模型目录用来回答"这个账号能用哪些模型"——模型列要靠它列出清单。
        listCatalogModels().catch(() => null),
      ]);
      setAccounts(accountResponse.data?.list ?? []);
      setChannels(channelResponse?.data ?? []);
      setProxies(proxyResponse?.data ?? []);
      setCatalogModels(catalogResponse?.data?.models ?? []);
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

  /*
    积分刷新：按钮立即刷，页面停留期间每 30s 自动刷一次。

    为什么不复用 load()：那是整表重载（账号 + 渠道 + 代理 + 模型目录四个请求，
    还会把 loading 蒙层盖回来）。积分刷新后只需要重拉一次账号列表 ——
    那个请求读的是网关内存快照（不打上游），轻且能带回刷新后的额度。
    其余列就地更新，不闪蒙层、不丢滚动位置。

    自动刷新只在「账号页打开」期间运行（effect 卸载即停），
    离开页面就不再消耗上游额度。
  */
  const [quotaPending, setQuotaPending] = useState(false);
  const quotaPendingRef = useRef(false);

  /** 只重拉账号列表，不触发整页 loading（额度/状态就地更新）。 */
  const reloadAccountsQuiet = useCallback(async () => {
    try {
      const response = await listAccounts({ pageSize: 200 });
      setAccounts(response.data?.list ?? []);
    } catch {
      // 静默失败：下一轮刷新或手动「刷新」会再拉。
    }
  }, []);

  const refreshQuotas = useCallback(async (silent = false) => {
    // ref 而不是 state 做防重入：state 置位是异步的，30s 轮询与按钮连点
    // 可能在同一个 tick 里双双通过检查。
    if (quotaPendingRef.current) {
      return;
    }
    quotaPendingRef.current = true;
    setQuotaPending(true);
    try {
      const response = await refreshAccountQuotas();
      const summary = response.data;
      await reloadAccountsQuiet();

      if (summary && summary.total > 0) {
        const failures = summary.results.filter((result) => !result.ok);
        if (summary.ok === 0) {
          // 全部失败通常意味着网关不可用。手动点按钮时明确提示；
          // 自动轮询保持安静，否则网关一挂每 30s 弹一次错。
          if (!silent) {
            toast.error(
              "积分刷新失败",
              failures[0]?.error ?? "网关不可用，请稍后重试",
            );
          }
        } else if (failures.length > 0 && !silent) {
          toast.info(
            "部分账号积分未刷新",
            `${failures.length} 个账号刷新失败，其余已更新`,
          );
        }
      }
    } catch (requestError) {
      // 自动轮询失败保持安静（下一轮会重试）；手动按钮才弹错。
      if (!silent) {
        toast.error("积分刷新失败", getRequestErrorMessage(requestError));
      }
    } finally {
      quotaPendingRef.current = false;
      setQuotaPending(false);
    }
  }, [reloadAccountsQuiet]);

  /*
    自动轮询：30s 间隔，静默模式（失败不弹提示）。
    窗口隐藏时跳过 —— 上游额度接口虽轻，页面根本看不见时刷它
    纯属浪费、也容易撞风控。
  */
  useEffect(() => {
    const AUTO_INTERVAL_MS = 30_000;

    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        void refreshQuotas(true);
      }
    }, AUTO_INTERVAL_MS);

    const onVisible = () => {
      // 切回页面立即刷一次，不等下一个 30s。
      if (document.visibilityState === "visible") {
        void refreshQuotas(true);
      }
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refreshQuotas]);

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    return accounts.filter((account) => {
      /*
        「待分配」也是一个可筛的分组。
        没有它，用户想看"哪些号还没归组"只能一页页翻、靠肉眼找空单元格。
      */
      if (channelFilter === UNASSIGNED) {
        if (account.channelId) {
          return false;
        }
      } else if (channelFilter !== ALL && account.channelId !== channelFilter) {
        return false;
      }
      if (!trimmed) {
        return true;
      }
      return (
        account.uid.toLowerCase().includes(trimmed) ||
        account.nickname.toLowerCase().includes(trimmed) ||
        account.channelId.toLowerCase().includes(trimmed) ||
        account.platform.toLowerCase().includes(trimmed) ||
        accountPlatformLabel(account).toLowerCase().includes(trimmed)
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
      try {
        await updateAccount(account.id, {});
        toast.success("凭据已刷新", account.nickname || account.uid);
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        toast.error("刷新凭据失败", getRequestErrorMessage(requestError));
      } finally {
        setBusy("");
      }
    },
    [],
  );

  /**
   * 打开测试弹窗。
   *
   * 不在按钮上直接发请求：上游限流按模型生效，
   * 必须让人先选模型 —— 否则测到的可能恰好是被限流的那个，
   * 得出"号废了"的错误结论。
   */
  const probe = useCallback((account: Account) => {
    setProbing(account);
  }, []);

  /**
   * 重置冷却：把账号级 + 模型级冷却一起清掉。
   *
   * `cleared=false` 说明本来就没在冷却 —— 如实提示，
   * 别让人以为"点了没生效"。
   */
  const resetCooldown = useCallback(async (account: Account) => {
    setBusy(`reset-${account.id}`);
    try {
      const response = await resetAccountCooldowns(account.id);
      if (response.data?.cleared) {
        toast.success("已重置冷却", "该账号可以立即参与调度");
      } else {
        toast.info("本来就没有冷却", "该账号当前可用");
      }
      setReloadKey((value) => value + 1);
    } catch (requestError) {
      toast.error("重置失败", getRequestErrorMessage(requestError));
    } finally {
      setBusy("");
    }
  }, []);

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

  /**
   * 批量分配分组。

   * 为什么需要：号可以先建、后归组 —— 尤其批量导入之后，
   * 逐个进编辑框改分组是纯粹的体力活。
   * `channelId` 传空串表示把选中的号全部摘回"待分配"。
   */
  const assignMany = useCallback(
    async (ids: string[], target: string) => {
      const targetChannel = channels.find((channel) => channel.id === target);
      if (target && (!targetChannel || ids.some((id) => {
        const account = accounts.find((item) => String(item.id) === id);
        return !account || !accountMatchesChannel(account, targetChannel);
      }))) {
        toast.error("无法分配", "所选账号必须与目标分组的平台及版本一致");
        return;
      }
      setBusy("assign");
      setError("");
      try {
        for (const key of ids) {
          const account = accounts.find((item) => String(item.id) === key);
          if (account) {
            await updateAccount(account.id, { channelId: target });
          }
        }
        setSelected(new Set());
        toast.success(
          target ? "已分配到分组" : "已解除分配",
          `${ids.length} 个账号`,
        );
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        toast.error("批量分配失败", getRequestErrorMessage(requestError));
        setReloadKey((value) => value + 1);
      } finally {
        setBusy("");
      }
    },
    [accounts, channels],
  );

  const columns: Array<Column<Account>> = useMemo(
    () => [
      {
        key: "account",
        header: "账号",
        width: "18%",
        /*
          昵称 + uid 都是可截断的长文本，给个下限让它不至于只剩几个字。
          上限靠 `width` 的百分比：宽度富余时不必让账号列独占空间。
        */
        minWidth: "200px",
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
        width: "152px",
        render: (account) => (
          <StatusBadge tone="neutral" label={accountPlatformLabel(account)} dot={false} />
        ),
      },
      {
        key: "channel",
        header: "分组",
        width: "124px",
        /*
          未分配分组显示成「待分配」，而不是空白。

          空白会被读成"这一列没数据/加载失败"；而"待分配"是一个**真实状态**：
          号已经建好、凭据可用，只是还没归到任何计价桶里，因此不参与对外调度。
        */
        render: (account) =>
          account.channelId ? (
            <span className="block truncate text-xs text-muted-foreground">
              {account.channelName || account.channelId}
            </span>
          ) : (
            <StatusBadge tone="warning" label="待分配" dot={false} />
          ),
      },
      {
        key: "quota",
        header: "额度",
        width: "152px",
        /*
          `7219.1 / 10.3k` 这一行一旦折行，"剩余 / 总量" 就分不清谁是谁了。
          下限 = 这行数字实测宽度（约 74px）+ 单元格左右内距（32px）+ 余量。
          不按进度条的 132px 定：那会让这一列白占 30px，挤掉账号列。
        */
        minWidth: "136px",
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
        header: "并发",
        width: "108px",
        align: "center",
        render: (account) => <CapacityCell account={account} />,
      },
      {
        key: "today",
        header: "今日",
        width: "152px",
        align: "center",
        hideOnMobile: true,
        render: (account) => {
          const usage = account.usage;
          if (!usage) {
            return <span className="text-xs text-muted-foreground">—</span>;
          }
          return (
            <div className="flex min-w-24 flex-col items-end whitespace-nowrap leading-4">
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
        key: "models",
        header: "模型",
        width: "132px",
        /*
          「查看模型」四个字 + 箭头在 12px 字号下约 64px，
          加上单元格左右内距，低于 120px 就会被折成「查看 / 模型」。
          限流后缀（「· 2 限流」）允许折行：那是附注，断了不影响理解。
        */
        minWidth: "120px",
        /*
          单独一列：状态回答"号能不能用"，这列回答"有哪些模型、哪些在限流"。
          合并会把"号还能跑其他模型"这个关键事实盖掉。
          点开是弹窗而不是行内展开：十几个模型铺在行里会把行高撑爆。
        */
        render: (account) => (
          <ModelListButton
            account={account}
            catalog={catalogModels}
            onOpen={() => setModelTarget(account)}
          />
        ),
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
                  /*
                    测试：用这个账号真实发一句 hi，确认凭据可用。

                    与"刷新凭据"是两件事：那个换 token，这个验证"现在能不能跑"。
                    账号显示"正常"但实际被上游挡了的情形，只有真发一次才看得出来。
                  */
                  label: "测试凭据",
                  id: "probe",
                  icon: FlaskConical,
                  disabled: busy === `probe-${account.id}`,
                  onClick: () => void probe(account),
                },
                {
                  /*
                    重置冷却：把账号级 + 模型级冷却一起清掉。

                    上游恢复得比退避策略早时，自动等待会让号白闲着。
                    没用冷却时点它是无害的 —— 会提示"本来就没在冷却"。
                  */
                  label: "重置冷却",
                  id: "reset",
                  icon: RotateCcw,
                  disabled: busy === `reset-${account.id}`,
                  onClick: () => void resetCooldown(account),
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
    [busy, refresh, probe, resetCooldown, toggleEnabled, catalogModels],
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
              {/* 待分配：账号是独立实体，允许先建号、之后再把号归到某个分组。 */}
              <SelectItem value={UNASSIGNED}>待分配（未归组）</SelectItem>
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
          <>
            {/*
              批量分配：选中若干号，一次归到某个分组（或摘回待分配）。

              放在删除旁边但不合并：分配是**高频操作**（导入后第一件事），
              删除是低频破坏性操作，放同一个下拉里会让误触风险翻倍。
            */}
            <Select
              value="__assign__"
              onValueChange={(next) => {
                if (next !== "__assign__") {
                  void assignMany([...selected], next === "__unassign__" ? "" : next);
                }
              }}
            >
              <SelectTrigger
                aria-label="批量分配分组"
                className="h-9 w-52"
                disabled={busy === "assign"}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__assign__">
                  分配到分组（{selected.size} 个账号）…
                </SelectItem>
                <SelectItem value="__unassign__">解除分配（回到待分配）</SelectItem>
                {channels.filter((channel) => channel.enabled && accounts.filter((account) => selected.has(String(account.id))).every((account) => accountMatchesChannel(account, channel))).map((channel) => (
                  <SelectItem key={channel.id} value={channel.id}>
                    {channel.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
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
          </>
        ) : null}

        <div className="ml-auto flex items-center gap-2">
          {/*
            刷新积分：强制打一次上游的额度接口（与读内存快照的「刷新」分开）。

            常驻巡检 5 分钟才更新一次快照，"刚做完任务积分怎么没变"
            就点这个。页面停留期间每 30s 也会自动刷一次。
          */}
          <Button
            variant="outline"
            size="sm"
            onClick={() => void refreshQuotas()}
            disabled={quotaPending}
          >
            <Coins
              className={cn(quotaPending && "animate-pulse motion-reduce:animate-none")}
              aria-hidden="true"
            />
            {quotaPending ? "刷新中..." : "刷新积分"}
          </Button>
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

      {/*
        测试弹窗：先选模型再发请求。

        上游限流按模型生效，直接测"默认模型"可能恰好撞在被限流的那个上，
        得出"号废了"的错误结论。测完刷新一次，让状态列与模型列跟上运行态。
      */}
      <ProbeDialog
        account={probing}
        onClose={() => {
          setProbing(null);
          setReloadKey((value) => value + 1);
        }}
      />

      {/* 模型清单：只读，不消耗额度，所以关闭时不必刷新。 */}
      <ModelListDialog
        account={modelTarget}
        catalog={catalogModels}
        onClose={() => setModelTarget(null)}
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
 * 并发：当前请求数 / 网关生效上限。
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

  return <span title={`当前请求 ${current} 个 / 生效并发上限 ${max} 个`}><StatusBadge tone={tone} label={`${current}/${max}`} dot={false} /></span>;
};
