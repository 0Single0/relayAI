import { ChevronDown, ChevronRight, Play, Square } from "lucide-react";
import type { TaskAccountLive, TaskScope } from "@/api/types";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { TaskRunLive } from "@/pages/Tasks/components/TaskRunLive";
import { cn } from "@/lib/utils";

type TaskAccountRowProps = {
  channelId: string;
  account: TaskAccountLive;
  scope: TaskScope;
  expanded: boolean;
  onToggle: () => void;
  onRun: () => void;
  onCancel: (runId: number) => void;
  onRunFinished: () => void;
  /** 本次（或最近一次）执行 id：展开时用 SSE 显示逐条结果。 */
  runId: number | null;
  busy: string;
};

const itemTone = (status: string): StatusTone => {
  switch (status) {
    case "done":
      return "success";
    case "failed":
      return "danger";
    case "skipped":
    case "not_in_window":
      return "neutral";
    case "claimable":
      return "info";
    default:
      return "neutral";
  }
};

const itemLabel = (status: string) => {
  switch (status) {
    case "done":
      return "已完成";
    case "failed":
      return "失败";
    case "skipped":
      return "跳过";
    case "not_in_window":
      return "不在窗口";
    case "claimable":
      return "可领取";
    case "not_accepted":
      return "未接取";
    case "in_progress":
      return "进行中";
    case "unsupported":
      return "不支持";
    default:
      return status;
  }
};

const progressOf = (account: TaskAccountLive) => {
  const total = account.tasks.length + account.daily.length;
  const done =
    account.tasks.filter((task) => task.status === "done").length +
    account.daily.filter((action) => action.doneToday).length;
  const dailyPending = account.daily.filter((action) => !action.doneToday).length;

  return {
    text: `${done}/${total} 项${dailyPending > 0 ? ` · ${dailyPending} 个动作待做` : ""}`,
    done,
    total,
  };
};

export const TaskAccountRow = ({
  account,
  scope,
  expanded,
  onToggle,
  onRun,
  onCancel,
  onRunFinished,
  runId,
  busy,
}: TaskAccountRowProps) => {
  const progress = progressOf(account);
  const isBusy = busy === "run" || busy === `cancel-${account.runId ?? ""}`;

  return (
    <>
      <tr className="border-b last:border-b-0">
        <td className="px-3 py-2">
          <button
            type="button"
            onClick={onToggle}
            aria-expanded={expanded}
            className="flex min-w-0 items-center gap-1.5 text-left hover:text-primary"
          >
            {expanded ? (
              <ChevronDown className="size-3.5 shrink-0" aria-hidden="true" />
            ) : (
              <ChevronRight className="size-3.5 shrink-0" aria-hidden="true" />
            )}
            <span className="min-w-0 truncate font-medium">
              {account.nickname || account.uid}
            </span>
            <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
              {account.uid}
            </span>
          </button>
        </td>

        <td className="px-3 py-2 text-xs text-muted-foreground tabular-nums">
          {progress.text}
        </td>

        <td className="hidden px-3 py-2 md:table-cell">
          {account.running ? (
            <StatusBadge tone="info" label="执行中" />
          ) : !account.credentialOk ? (
            <StatusBadge tone="warning" label="无凭据" />
          ) : !account.enabled ? (
            <StatusBadge tone="neutral" label="已停用" />
          ) : (
            <StatusBadge tone="neutral" label="空闲" />
          )}
        </td>

        <td className="px-3 py-2">
          <div className="flex items-center justify-end gap-1">
            {account.running && account.runId !== null ? (
              <Button
                variant="outline"
                size="xs"
                disabled={isBusy}
                onClick={() => onCancel(account.runId as number)}
              >
                <Square aria-hidden="true" />
                取消
              </Button>
            ) : (
              <Button
                variant="outline"
                size="xs"
                disabled={isBusy || !account.enabled}
                onClick={onRun}
              >
                <Play aria-hidden="true" />
                {scope === "checkin" ? "签到" : "执行"}
              </Button>
            )}
          </div>
        </td>
      </tr>

      {expanded ? (
        <tr className="border-b last:border-b-0 bg-muted/20">
          <td colSpan={4} className="px-3 py-3">
            {account.error ? (
              <p className="mb-2 break-words text-xs text-destructive">
                {account.error}
              </p>
            ) : null}

            {/* 本次执行的结果在上：它是"刚发生了什么"，比清单更需要第一眼看到 */}
            {runId !== null ? (
              <TaskRunLive runId={runId} onFinished={onRunFinished} />
            ) : null}

            {/* 每日动作在上：与执行顺序一致（先每日动作，再任务清单） */}
            {account.daily.length > 0 ? (
              <div className="mb-3">
                <p className="mb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
                  每日动作
                </p>
                <ul className="space-y-1">
                  {account.daily.map((action) => (
                    <li
                      key={action.code}
                      className="flex flex-wrap items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 text-xs"
                    >
                      <span className="min-w-0 flex-1 truncate font-medium">
                        {action.name}
                      </span>
                      <StatusBadge
                        tone={action.doneToday ? "success" : "neutral"}
                        label={action.doneToday ? "今天已做" : action.action}
                      />
                      {action.note ? (
                        <span className="min-w-0 basis-full truncate text-[11px] text-muted-foreground">
                          {action.note}
                        </span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            {account.tasks.length > 0 ? (
              <div>
                <p className="mb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
                  任务清单
                </p>
                <ul className="space-y-1">
                  {account.tasks.map((task) => (
                    <li
                      key={task.code}
                      className={cn(
                        "flex flex-wrap items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 text-xs",
                        !task.automatable && "opacity-80",
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate font-medium">
                        {task.name}
                      </span>
                      <span className="w-14 shrink-0 text-right tabular-nums text-muted-foreground">
                        {task.progress || "—"}
                      </span>
                      <span className="w-12 shrink-0 text-right tabular-nums text-success">
                        {task.reward > 0 ? `+${task.reward}` : "—"}
                      </span>
                      <StatusBadge
                        tone={itemTone(task.status)}
                        label={itemLabel(task.status)}
                      />
                      {!task.automatable ? (
                        <StatusBadge tone="warning" label="需手动" dot={false} />
                      ) : null}
                      {task.note ? (
                        <span className="min-w-0 basis-full truncate text-[11px] text-muted-foreground">
                          {task.note}
                        </span>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            {account.daily.length === 0 && account.tasks.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                该账号没有可展示的任务或每日动作。
              </p>
            ) : null}
          </td>
        </tr>
      ) : null}
    </>
  );
};
