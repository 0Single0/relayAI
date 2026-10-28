import { Flame, Fuel, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import type { GrowthSnapshot } from "@/api/types";
import type { TaskRow } from "@/pages/Tasks/taskRows";

/**
 * 顶部状态条：**一行**展示成长数据与每日动作状态。
 *
 * 为什么必须是一行：
 *   这四格是同一层信息（"我现在什么水平、今天做没做"），
 *   分两排会让视觉重心散掉，也让下方的表格莫名其妙地被推低。
 *
 * 为什么用「一个容器 + 1px 分隔」而不是四个独立小卡片：
 *   四个浮动方块看起来像四个不相关的组件；
 *   一个连续的条带则读作「一组状态」，与仪表盘的处理一致。
 *   `gap-px` + 父级背景色是最省事的实心分隔线写法，
 *   不必给每格写 border（那样角上会出现双线）。
 */

/** 一格的内容定义。 */
type Cell = {
  key: string;
  icon: LucideIcon;
  label: string;
  /** 主值：数字或"已完成"这类短状态。 */
  value: string;
  /** 值的主色（已完成绿、需手工黄等）。 */
  tone?: string;
  /** 副文案：一句说明。 */
  hint?: string;
};

const StatusCell = ({ cell }: { cell: Cell }) => {
  const Icon = cell.icon;
  return (
    <div className="flex min-w-0 flex-col gap-0.5 bg-card px-3 py-2.5">
      <span className="flex items-center gap-1 text-[11px] text-muted-foreground">
        <Icon className="size-3 shrink-0" aria-hidden="true" />
        <span className="truncate">{cell.label}</span>
      </span>
      <span className={cn("text-sm font-medium tabular-nums", cell.tone)}>{cell.value}</span>
      {cell.hint ? (
        <span className="truncate text-[10px] text-muted-foreground" title={cell.hint}>
          {cell.hint}
        </span>
      ) : null}
    </div>
  );
};

/** 把一条每日动作压成格子。 */
const actionCell = (row: TaskRow): Cell => {
  const allDone = row.doneCount === row.accountCount && row.accountCount > 0;
  const note = row.accounts[0]?.note ?? "";
  const detail = row.accounts
    .map((item) => `${item.nickname || item.uid}：${item.note || item.progress}`)
    .join("\n");

  return {
    key: `action-${row.code}`,
    icon: allDone ? Flame : Fuel,
    label: row.name,
    value: allDone ? "已完成" : row.automatable ? "待执行" : "需手工",
    tone: allDone ? "text-success" : row.automatable ? "" : "text-warning",
    hint: note || detail,
  };
};

export const TaskStats = ({
  growth,
  actionRows,
}: {
  growth: GrowthSnapshot | null;
  actionRows: TaskRow[];
}) => {
  const cells: Cell[] = [];

  // 成长数据只在国内版有；国际版只有每日动作，这里自然就只剩动作格。
  if (growth) {
    cells.push({
      key: "energy",
      icon: Fuel,
      label: "加油站能量",
      value: growth.energy === null ? "—" : String(growth.energy),
      hint: growth.energy === null ? "上游未返回" : undefined,
    });
    cells.push({
      key: "streak",
      icon: Flame,
      label: "连续登录",
      value: growth.streakDays === null ? "—" : `${growth.streakDays} 天`,
      hint: growth.streakDays === null ? "上游未返回" : undefined,
    });
  }

  cells.push(...actionRows.map(actionCell));

  if (cells.length === 0) {
    return null;
  }

  return (
    <div
      className={cn(
        "grid overflow-hidden rounded-lg border",
        // 窄屏两列；宽屏一律一行 —— 无论 2 格还是 4 格。
        "grid-cols-2 gap-px bg-border",
        cells.length >= 4 ? "sm:grid-cols-4" : cells.length === 3 ? "sm:grid-cols-3" : "sm:grid-cols-2",
      )}
    >
      {cells.map((cell) => (
        <StatusCell key={cell.key} cell={cell} />
      ))}
    </div>
  );
};
