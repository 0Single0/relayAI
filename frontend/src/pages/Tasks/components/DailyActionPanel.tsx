import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { TaskRow } from "@/pages/Tasks/taskRows";

/**
 * 每日动作（签到 / 猫猫旅行 / 抽奖…）。
 *
 * 两部分，职责分开：
 *   1. **按钮在顶部操作栏**（与刷新、一键执行同一行）——
 *      它们是"要动手做的事"，和刷新属于同一类操作；
 *   2. **状态卡在下面**——只读，回答"现在什么情况"。
 *
 * 为什么按钮必须提到顶部：
 *   用户的心智是"打开页面 → 点执行"。按钮埋在内容区里，
 *   会和"一键执行"重复且分散注意力。而一键执行**本来就包含**这些动作
 *   （scope=all 会先跑每日动作再跑任务），所以不该再出现
 *   第二个"全部执行"按钮。
 *
 * 视觉对齐仪表盘：用 `bg-muted/50` 的填充块，不靠描边分割。
 */

/** 只读状态卡。 */
const StatCard = ({ row }: { row: TaskRow }) => {
  const allDone = row.doneCount === row.accountCount && row.accountCount > 0;
  const note = row.accounts[0]?.note ?? "";
  const detail = row.accounts
    .map((item) => `${item.nickname || item.uid}：${item.note || item.progress}`)
    .join("\n");

  const status = allDone ? "已完成" : row.automatable ? "待执行" : "需手工";

  return (
    <div
      className="flex min-w-0 flex-col gap-0.5 rounded-lg bg-muted/50 px-2.5 py-1.5"
      title={detail || note}
    >
      <span className="truncate text-[11px] text-muted-foreground">{row.name}</span>
      <span
        className={cn(
          "text-sm font-medium",
          allDone ? "text-success" : row.automatable ? "" : "text-warning",
        )}
      >
        {status}
      </span>
      <span className="truncate text-[10px] text-muted-foreground">
        {note || `${row.doneCount}/${row.accountCount} 账号`}
      </span>
    </div>
  );
};

/** 状态卡区（只读）。 */
export const DailyActionStats = ({ rows }: { rows: TaskRow[] }) => {
  if (rows.length === 0) {
    return null;
  }
  return (
    <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
      {rows.map((row) => (
        <StatCard key={row.key} row={row} />
      ))}
    </div>
  );
};

/**
 * 每日动作的执行按钮（放在顶部操作栏里）。
 *
 * 这里**不加「全部执行」**：顶部已有「一键执行」，它包含每日动作。
 * 两个"全执行"按钮并排只会让人不知道该点哪个。
 */
export const DailyActionButtons = ({
  rows,
  runningCodes,
  onRun,
}: {
  rows: TaskRow[];
  runningCodes: string[];
  onRun: (codes: string[]) => void;
}) => {
  if (rows.length === 0) {
    return null;
  }

  return (
    <>
      {rows.map((row) => {
        const running = runningCodes.includes(row.code);
        return (
          <Button
            key={row.key}
            variant="outline"
            size="sm"
            className="cursor-pointer"
            disabled={running || !row.automatable}
            onClick={() => onRun([row.code])}
            title={row.accounts[0]?.note ?? ""}
          >
            {running ? `${row.name}执行中` : row.name}
          </Button>
        );
      })}
    </>
  );
};
