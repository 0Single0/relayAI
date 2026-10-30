import { ListChecks, Play, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import {
  cancelTaskRun,
  getTaskChannelLive,
  getTaskRun,
  startTaskRun,
} from "@/api/tasks";
import type { TaskChannelLive, TaskScope } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { toast } from "@/components/common/Toast/store";
import { EmptyState } from "@/components/common/EmptyState";
import { DailyActionButtons } from "@/pages/Tasks/components/DailyActionPanel";
import { TaskStats } from "@/pages/Tasks/components/TaskStats";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
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
import { buildTaskRows, dailyProgress, type TaskRow } from "@/pages/Tasks/taskRows";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const POLL_INTERVAL_MS = 5000;
const ALL_ACCOUNTS = "__all__";
/** 整批执行的占位码（codes 为空时代表全部）。 */
const ALL_CODES = "__all_codes__";

const statusLabel = (row: TaskRow) => {
  if (row.doneCount === row.accountCount) {
    return "已完成";
  }
  if (row.doneCount === 0) {
    return "未开始";
  }
  return "进行中";
};

const statusTone = (row: TaskRow): StatusTone => {
  if (row.doneCount === row.accountCount) {
    return "success";
  }
  if (row.doneCount === 0) {
    return "neutral";
  }
  return "warning";
};

/**
 * 渠道任务页。
 *
 * **主体是任务，不是账号。** 这一页回答"今天有哪些任务、各自完成到哪"，
 * 所以一行 = 一条任务（跨账号聚合），账号退化成归属维度：
 *   · 「账号完成」列显示 `已完成/总数`，展开能看到是哪些号；
 *   · 顶部有账号下拉，可只执行某个账号的任务；
 *   · 也可以一键执行全部账号。
 *
 * 为什么不做成"按账号列表"：同一个任务在 20 个账号下会重复出现 20 次，
 * 用户看不出全局进度，而"今天还有几个号没签到"才是他要的答案。
 */
export const TaskChannelPage = () => {
  /*
    URL 参数是**版本**（`cn` / `intl`），不是分组 id。

    任务是一种产品能力，不该因为没建分组就没入口。版本决定显示哪套内容：
    国内版有成长任务 + 签到 + 猫猫旅行 + 抽奖，国际版只有每日活跃打卡。

    **取号也按版本**，不按分组：账号是独立实体，可以还没分配分组，
    也可以和别的分组混在同一版本里。只按一个分组取号，未分配分组的号
    永远不会出现在这一页 —— 而那正是用户最需要看到"它还没归组"的地方。
  */
  const { channelId = "cn" } = useParams<{ channelId: string }>();
  const realm = channelId === "intl" ? "intl" : "cn";

  const [live, setLive] = useState<TaskChannelLive | null>(null);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [accountSelection, setAccountSelection] = useState({ realm: "", uid: ALL_ACCOUNTS });
  const accountFilter = accountSelection.realm === realm ? accountSelection.uid : ALL_ACCOUNTS;
  const setAccountFilter = useCallback((uid: string) => setAccountSelection({ realm, uid }), [realm]);

  /*
    深链接：`/tasks/cn?account=<uid>` 直接定位到某个号。

    账号详情页的「查看该账号的任务」用它。之前这个参数没人读 ——
    链接跳过来仍然停在"全部账号"，用户还得自己在下拉里再找一次那个号。
  */
  const [searchParams] = useSearchParams();
  const linkedAccount = searchParams.get("account") ?? "";
  useEffect(() => {
    if (linkedAccount) {
      setAccountSelection({ realm, uid: linkedAccount });
    }
  }, [linkedAccount, realm]);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const requestSequence = useRef(0);
  const requestController = useRef<AbortController | null>(null);

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    requestController.current?.abort();
    const controller = new AbortController();
    requestController.current = controller;
    setIsLoading(true);
    try {
      const response = await getTaskChannelLive(realm, controller.signal);
      if (sequence !== requestSequence.current || controller.signal.aborted) return;
      setLive(response.data ?? null);
      setError("");
    } catch (requestError) {
      if (sequence !== requestSequence.current || controller.signal.aborted) return;
      setError(getRequestErrorMessage(requestError));
      setLive(null);
    } finally {
      if (sequence === requestSequence.current) setIsLoading(false);
    }
  }, [realm]);

  useEffect(() => {
    void load();
    const controller = requestController.current;
    return () => {
      controller?.abort();
    };
  }, [load, reloadKey, accountFilter]);


  const accounts = useMemo(() => live?.accounts ?? [], [live]);

  /** 只保留选中账号的数据（先筛账号，再聚合，保证"任务"这一层口径不变）。 */
  const visibleAccounts = useMemo(() => {
    if (accountFilter === ALL_ACCOUNTS) {
      return accounts;
    }
    return accounts.filter((account) => account.uid === accountFilter);
  }, [accountFilter, accounts]);

  /**
   * 每日动作与成长任务**分成两份数据**。
   *
   * 前者渲染成卡片（每天要看的那几项），后者留在表格里（一次性清单）。
   * 判断依据是 `kind`，由 `buildTaskRows` 给好。
   */
  const actionRows = useMemo(
    () => buildTaskRows(visibleAccounts).filter((row) => row.kind === "daily"),
    [visibleAccounts],
  );
  const taskRows = useMemo(
    () => buildTaskRows(visibleAccounts).filter((row) => row.kind === "task"),
    [visibleAccounts],
  );

  const daily = useMemo(() => dailyProgress(actionRows), [actionRows]);

  const anyRunning = useMemo(
    () => accounts.some((account) => account.running),
    [accounts],
  );

  useEffect(() => {
    if (!anyRunning) {
      return;
    }
    const timer = window.setInterval(() => {
      void load();
    }, POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [anyRunning, load]);

  const scope = useMemo<TaskScope>(() => {
    if (accounts.length === 0) {
      return "all";
    }
    const hasTasks = accounts.some((account) => account.tasks.length > 0);
    const hasDaily = accounts.some((account) => account.daily.length > 0);
    return !hasTasks && !hasDaily ? "checkin" : "all";
  }, [accounts]);

  /**
   * 等这一批 run 跑完，然后用 toast 汇报结果。
   *
   * 为什么要这一步：之前点了只弹一句"已开始执行"，然后就没了 ——
   * 用户完全不知道最后成了没有。执行是异步的（网关后台跑），
   * 所以这里轮询到终态再一次性给出「成功 N / 失败 M / 跳过 K / 得分 X」。
   *
   * 每 2 秒查一次、最多等 3 分钟：
   *   · 上限是因为一个账号几十项任务、每项之间还有间隔；
   *   · 超时不算失败，提示"仍在执行"并让用户看表格。
   */
  const reportRunOutcome = useCallback(async (runIds: number[]) => {
    if (runIds.length === 0) {
      return;
    }
    const deadline = Date.now() + 3 * 60 * 1000;
    let done = 0;
    let failed = 0;
    let skipped = 0;
    let earned = 0;
    let pending = runIds.length;

    while (Date.now() < deadline && pending > 0) {
      await new Promise((resolve) => window.setTimeout(resolve, 2000));
      done = 0;
      failed = 0;
      skipped = 0;
      earned = 0;
      pending = 0;
      let lastError = "";

      for (const runId of runIds) {
        try {
          const response = await getTaskRun(runId);
          const run = response.data?.run;
          if (!run) {
            continue;
          }
          done += run.doneCount;
          failed += run.failedCount;
          skipped += run.skippedCount;
          earned += run.earned;
          if (run.error) {
            lastError = run.error;
          }
          if (run.status === "running") {
            pending += 1;
          }
        } catch {
          // 单条查询失败不影响其余：下一轮会重试到终态。
          pending += 1;
        }
      }

      if (pending === 0) {
        /*
          额度变化要说清楚，包括"没涨"。

          之前只在 `earned > 0` 时显示"得分 N"，等于把最关键的
          "这次到底多没多积分"藏起来了 —— 用户执行完最想知道的就是这个。
          现在无论涨没涨都给一句明确结论。
        */
        const gain =
          earned > 0 ? `+${earned} 积分` : "本次没有新增积分";

        const detailParts = [
          `成功 ${done}`,
          failed > 0 ? `失败 ${failed}` : "",
          skipped > 0 ? `跳过 ${skipped}` : "",
        ].filter(Boolean);
        const summary = `${gain} · ${detailParts.join(" · ")}`;

        if (failed > 0 || lastError) {
          toast.error(
            earned > 0 ? `执行完成，获得 +${earned} 积分` : "执行完成，但有失败",
            `${summary}${lastError ? `\n${lastError}` : ""}`,
          );
        } else if (done === 0 && skipped > 0) {
          toast.info("执行完成，全部跳过", summary);
        } else {
          toast.success(
            earned > 0 ? `执行完成，获得 +${earned} 积分` : "执行完成，未新增积分",
            summary,
          );
        }
        setReloadKey((value) => value + 1);
        return;
      }
    }

    toast.info("仍在执行", "任务还在后台跑，稍后刷新可看结果");
    setReloadKey((value) => value + 1);
  }, []);
  /**
   * 正在执行的任务码集合。
   *
   * **按任务独立**，不是全局一个 busy 标志：
   * 点「猫猫旅行」只该让那一张卡变灰，其它任务照样能点。
   * 早先用单一 busy 字符串时，点任意一条都会把整张表置灰 ——
   * 看起来像"系统忙"，实际上是人为限制了并发。
   */
  const [runningCodes, setRunningCodes] = useState<string[]>([]);

  const runAccounts = useCallback(
    async (uids: string[], codes: string[] = []) => {
      if (uids.length === 0) {
        toast.info(
          "没有可执行的账号",
          "该任务的所有账号都已完成，或凭据不可用",
        );
        return;
      }
      /*
        每个账号带上它**自己**的分组。

        这一页按版本取号，同屏可能有多个分组、也可能有还没分配分组的号
        （`channelId` 是空串）。统一用一个"代表分组"会把待分配的号
        发到错误的分组上 —— 网关那边就找不到账号了。
      */
      const targets = uids.map((uid) => ({
        uid,
        channelId: accounts.find((account) => account.uid === uid)?.channelId ?? "",
      }));
      // 只标记这次涉及的任务码；空 codes 表示整批，用一个哨兵值占位。
      const marker = codes.length > 0 ? codes : [ALL_CODES];
      setRunningCodes((prev) => [...new Set([...prev, ...marker])]);
      try {
        const response = await startTaskRun({
          accounts: targets,
          scope,
          codes,
        });
        const runIds = (response.data?.runs ?? []).map((item) => item.runId);
        toast.success(
          "已开始执行",
          codes.length > 0
            ? `${codes.length} 项任务 · ${uids.length} 个账号`
            : `${uids.length} 个账号`,
        );
        // 立刻刷新一次，让"执行中"状态可见；随后由轮询接手汇报结果。
        setReloadKey((value) => value + 1);
        await reportRunOutcome(runIds);
      } catch (requestError) {
        toast.error("执行失败", getRequestErrorMessage(requestError));
      } finally {
        setRunningCodes((prev) => prev.filter((code) => !marker.includes(code)));
      }
    },
    [accounts, scope, reportRunOutcome],
  );

  const targetAccounts = useMemo(
    () => (accountFilter === ALL_ACCOUNTS ? accounts : visibleAccounts),
    [accountFilter, accounts, visibleAccounts],
  );

  const primaryLabel =
    accountFilter === ALL_ACCOUNTS ? "一键执行全部账号" : "执行该账号的任务";

  const runningRunId = useMemo(() => {
    for (const account of accounts) {
      if (account.running && account.runId) {
        return account.runId;
      }
    }
    return null;
  }, [accounts]);

  const columns: Array<Column<TaskRow>> = useMemo(
    () => [
      {
        key: "name",
        header: "任务",
        width: "30%",
        bodyAlign: "left",
        render: (row) => (
          <div className="min-w-0">
            <p className="truncate font-medium">{row.name}</p>
            <p className="truncate font-mono text-[11px] text-muted-foreground">
              {row.kind === "daily" ? "每日动作" : "任务"} · {row.code}
            </p>
          </div>
        ),
      },
      {
        key: "accounts",
        header: "账号完成",
        width: "112px",
        /*
          账号明细放在悬停提示里，而不是展开一行。

          为什么去掉折叠：顶部已经有账号筛选器，"这个任务哪些号完成了"
          用一句 title 就能答完；为它多占一列 + 一次点击，等于让每行都要
          点一下才知道结论。折叠还让行高跳动，扫视时很烦。
        */
        render: (row) => {
          const detail = row.accounts
            .map(
              (item) =>
                `${item.nickname || item.uid}${item.channelId ? "" : "（待分配）"}：${
                  item.status === "done" ? "已完成" : `未完成（${item.progress}）`
                }`,
            )
            .join("\n");
          return (
            <span className="tabular-nums text-xs" title={detail}>
              {row.doneCount}/{row.accountCount}
            </span>
          );
        },
      },
      {
        key: "status",
        header: "状态",
        width: "96px",
        render: (row) => (
          <StatusBadge tone={statusTone(row)} label={statusLabel(row)} dot={false} />
        ),
      },
      {
        key: "automatable",
        header: "可自动化",
        width: "96px",
        hideOnMobile: true,
        render: (row) =>
          row.automatable ? (
            <StatusBadge tone="info" label="可执行" dot={false} />
          ) : (
            <span className="text-xs text-muted-foreground">需手工</span>
          ),
      },
      {
        key: "actions",
        header: "操作",
        width: "96px",
        align: "center",
        render: (row) => (
          <RowActions
            actions={[
              {
                id: "run",
                // 不可自动化的任务给一个明确原因，而不是让按钮变成"点了没反应"。
                label: row.automatable ? "执行此任务" : "该任务需手工完成",
                icon: Play,
                tone: "success",
                disabled: !row.automatable || runningCodes.includes(row.code) || runningCodes.includes(ALL_CODES),
                onClick: () => {
                  /*
                    执行**这一条**任务，而不是整批。

                    之前两处都错了：
                      · 没把 `row.code` 传下去 —— 点"猫猫旅行"实际触发了全部任务；
                      · 先按 `status !== "done"` 过滤账号 —— 今天已完成时 uids 为空，
                        `runAccounts([])` 直接 return，表现就是"点了没反应"。
                    现在即使全部已完成也照常发起：让后端回一条"今天已完成"的
                    skipped 结果，用户至少知道发生了什么。
                  */
                  const uids = row.accounts
                    .filter((item) => item.enabled && item.credentialOk)
                    .map((item) => item.uid);
                  void runAccounts(uids, [row.code]);
                },
              },
            ]}
          />
        ),
      },
    ],
    [runningCodes, runAccounts],
  );

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-44 shrink-0">
          <Select value={accountFilter} onValueChange={setAccountFilter}>
            <SelectTrigger aria-label="选择账号">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL_ACCOUNTS}>全部账号（{accounts.length}）</SelectItem>
              {accounts.map((account) => (
                <SelectItem key={account.uid} value={account.uid}>
                  {/* 未分配分组的号也在这里，标出来才不会让人以为"它归了某个组"。 */}
                  {account.nickname || account.uid}
                  {account.channelId ? "" : "（待分配）"}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <span className="text-xs text-muted-foreground">
          {realm === "intl" ? (
            // 国际版没有成长任务体系（参考实现在入口处直接拒掉），
            // 只有每日活跃打卡。这里说清楚，避免用户以为"任务丢了"。
            <>国际版 · 每日活跃打卡 {daily.done}/{daily.total}（无成长任务）</>
          ) : (
            <>
              共 {taskRows.length} 项成长任务
            </>
          )}
        </span>

        <div className="ml-auto flex flex-wrap items-center gap-2">
          {
            /*
              每日动作按钮：与刷新、一键执行同一行。

              为什么提到这里：用户的心智是"打开页面 → 点执行"。
              按钮埋在内容区里会和「一键执行」重复且分散注意力 ——
              而一键执行本来就包含这些动作（scope=all 会先跑每日动作）。
              所以这里只放单条动作按钮，不再有第二个"全部执行"。
            */
          }
          <DailyActionButtons
            rows={actionRows}
            runningCodes={runningCodes}
            onRun={(codes) => {
              const uids = targetAccounts
                .filter((account) => account.enabled && account.credentialOk)
                .map((account) => account.uid);
              void runAccounts(uids, codes);
            }}
          />

          {runningRunId ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                void cancelTaskRun(runningRunId)
                  .then(() => toast.info("已请求取消", "正在飞的上游请求会跑完，后续项不再执行"))
                  .catch((requestError) =>
                    toast.error("取消失败", getRequestErrorMessage(requestError)),
                  );
              }}
            >
              停止执行
            </Button>
          ) : null}
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
          <Button
            size="sm"
            onClick={() => setConfirmOpen(true)}
            disabled={runningCodes.length > 0 || targetAccounts.length === 0}
          >
            <Play aria-hidden="true" />
            {primaryLabel}
          </Button>
        </div>
      </div>


      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
          /*
            别把"这个版本还没配账号"说成网关故障。

            绝大多数情况下这里失败是因为分组下没有号，而不是网关挂了。
            提示写成"确认网关已启动"会把人引向错误方向，白排查一轮。
          */
          hint="常见原因：该版本还没有可用账号，或账号凭据不可用。"
        />
      ) : null}
      <MyBox isLoading={isLoading} text="" className="shrink-0">
        <TaskStats
          accounts={visibleAccounts}
          showGrowth={realm === "cn"}
          actionRows={actionRows}
        />
      </MyBox>

      <MyBox
        /*
          条件只看 `isLoading`，不再要求 `live === null`。

          之前写成 `isLoading && live === null`：首屏有 spinner，
          但从国内版切到国际版（或点刷新）时 `live` 还留着上一次的数据，
          于是**完全没有加载提示** —— 页面像卡住了，用户不知道在转圈。

          现在每次请求都盖一层 spinner，代价只是短暂遮住旧数据，
          换来的是"我点的操作有反应"这个确认。
        */
        isLoading={isLoading}
        text="正在读取任务清单..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={taskRows}
          rowKey={(row) => row.key}
          empty={
            <EmptyState
              icon={ListChecks}
              title={accounts.length === 0 ? "还没有该版本的账号" : "今天没有任务"}
              description={
                accounts.length === 0
                  ? "国内版 / 国际版的号都会显示在这里，包括还没分配分组的号。先添加一个账号。"
                  : "账号已就绪，但上游当前没有返回待办任务。"
              }
              action={
                accounts.length === 0 ? (
                  <Button size="sm" asChild>
                    <Link to="/accounts">去添加账号</Link>
                  </Button>
                ) : undefined
              }
            />
          }
        />
      </MyBox>


      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>{primaryLabel}</DialogTitle>
            <DialogDescription>
              将对 {targetAccounts.length} 个账号依次执行（账号之间串行，避免触发风控）。
            </DialogDescription>
          </DialogHeader>

          <ul className="space-y-1 text-xs text-muted-foreground">
            <li>
              · 范围：
              {scope === "checkin" ? "仅签到" : "每日动作 + 任务清单（先每日动作）"}
            </li>
            <li>· 预计耗时：数分钟（每项之间有间隔）</li>
            <li className="text-warning">· 会真实消耗上游额度，并可能触发上游风控</li>
          </ul>

          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setConfirmOpen(false)}>
              取消
            </Button>
            <Button
              size="sm"
              onClick={() => {
                setConfirmOpen(false);
                void runAccounts(targetAccounts.map((account) => account.uid));
              }}
            >
              确认执行
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};


