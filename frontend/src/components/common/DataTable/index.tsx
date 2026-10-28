import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export type Column<T> = {
  /** 列标识，同时用作 React key。 */
  key: string;
  header: string;
  /** 列宽（CSS 值）。 */
  width?: string;
  /**
   * 对齐方式，**默认居中**。
   *
   * 为什么默认居中：后台表格里绝大多数格是短标签、数字、图标按钮，
   * 居中能让每列形成一条视觉中轴，扫视成本最低。
   */
  align?: "left" | "right" | "center";
  /**
   * 仅作用于**单元格内容**的对齐，不传则跟随 `align`。
   *
   * 存在的理由：有些列的表头是短词（"用户"），居中才好看；
   * 而内容是多行长文本（昵称 + 邮箱），必须左对齐才读得整齐。
   * 一个 `align` 同时管两处就做不到这件事 ——
   * 要么表头歪，要么内容参差。
   */
  bodyAlign?: "left" | "right" | "center";
  /** 单元格渲染。 */
  render: (row: T) => ReactNode;
  /** 窄屏是否隐藏（保留最重要的几列）。 */
  hideOnMobile?: boolean;
};

export type DataTableProps<T> = {
  columns: Array<Column<T>>;
  rows: T[];
  rowKey: (row: T) => string;
  onRowClick?: (row: T) => void;
  empty: ReactNode;
  className?: string;
  /**
   * 是否固定列宽（`table-fixed`）。
   *
   * 默认 `false`：自动布局下浏览器按内容分配宽度，短列不会被拉宽，
   * 大多数表格这样更好看。
   *
   * 但列多、且各列内容长度差异大时（使用记录那种 11 列的表），
   * 自动布局会把某一列按最长内容撑开、把其它列压扁 ——
   * 那种情况必须开 `fixed`，让 `width` 真正生效。
   */
  fixed?: boolean;
  /**
   * 是否显示斑马纹。
   *
   * 行内容以多行小块为主时（使用记录那种），纯白底会让眼睛"串行" ——
   * 隔行浅底是最省事的分隔手段，比加分割线轻。
   */
  striped?: boolean;
  /** 开启多选：传入选中的 key 集合与变更回调。 */
  selection?: {
    selected: Set<string>;
    onChange: (next: Set<string>) => void;
    /** 行是否可选（默认全部可选）。 */
    isSelectable?: (row: T) => boolean;
    /** 无障碍名称，如「选择账号 拾忆」。 */
    label?: (row: T) => string;
  };
};

/**
 * 数据表格（逐项复刻 FastGPT 的表格 UI）。
 *
 * 源码依据：`projects/app/src/pageComponents/account/team/GroupManage/index.tsx`
 * 的 `TableContainer` / `Table` / `Thead` / `Th`，配合
 * `packages/web/styles/theme.ts` 的 `Table` 定义与 `myGray` 调色板。
 *
 * | 项 | FastGPT 源码 | 这里 |
 * | --- | --- | --- |
 * | 外层容器 | `TableContainer overflow={'unset'}`，**无边框无圆角** | 无边框 |
 * | 表头底色 | 落在**每个 `Th`** 上（`bg="myGray.100"`） | 每个 `th` 上 `bg-muted` |
 * | 表头首尾圆角 | `borderLeftRadius="6px"` / `borderRightRadius="6px"` | 首列 `rounded-l-md`、末列 `rounded-r-md` |
 * | 表头字体 | `text-xs` **常规字重**（Chakra `Th` 默认 `fontWeight=normal`） | `text-xs font-normal` |
 * | 表头颜色 | `myGray.500`（次要文字） | `text-muted-foreground` |
 * | 数据字号 | `fontSize={'sm'}`（14px） | `text-sm` |
 * | 左右内距 | `px-4` | `px-4` |
 * | 行分隔线 | 单条 `#E8EBF0` | `border-b border-border` |
 * | 竖线 | **无** | 无 |
 * | 行高 | 由 padding 决定 | `py-3` |
 *
 * 三个刻意的决定：
 *   · **表头底色只铺在文字格上**（不是整条 tr）。FastGPT 的做法让表头看起来像
 *     一排"胶囊"，首尾有圆角；整条铺满会变成一条直边色带，观感差很远。
 *   · **不加外边框**。表头底色 + 行分隔线已经足够表达结构，再套一圈 border
 *     会显得又厚又重。
 *   · **行高由 padding 决定**，不写死 `h-10`：两行文本的单元格会被压扁。
 *
 * 容器是 `flex-1 overflow-auto`：表格自己内部滚动，页面不跟着滚。
 * 这样分页条能稳定停在右下角，不会被表格内容推走。
 */
export function DataTable<T>({
  columns,
  rows,
  rowKey,
  onRowClick,
  empty,
  className,
  fixed = false,
  striped = false,
  selection,
}: DataTableProps<T>) {
  if (rows.length === 0) {
    return <>{empty}</>;
  }

  const selectableRows = selection
    ? rows.filter((row) => selection.isSelectable?.(row) ?? true)
    : [];
  const allSelected =
    selectableRows.length > 0 &&
    selectableRows.every((row) => selection?.selected.has(rowKey(row)));

  const toggleAll = () => {
    if (!selection) {
      return;
    }
    const next = new Set(selection.selected);
    if (allSelected) {
      for (const row of selectableRows) {
        next.delete(rowKey(row));
      }
    } else {
      for (const row of selectableRows) {
        next.add(rowKey(row));
      }
    }
    selection.onChange(next);
  };

  const toggleOne = (key: string) => {
    if (!selection) {
      return;
    }
    const next = new Set(selection.selected);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    selection.onChange(next);
  };

  return (
    <div className={cn("min-h-0 flex-1 overflow-auto", className)}>
      {/*
        **不要给 table 加 `h-full`**：`<table>` 是 `display: table`，
        加了 `h-full` 之后浏览器会把剩余高度**均摊到各个数据行**上（把行拉高），
        而不是留白；再配合下方分页条就会出现"表格压住分页"（实测重叠）。
        正确做法是让表格保持内容高度、由外层 `overflow-auto` 承担滚动，
        需要撑满观感时用 `tbody` 末尾的吸收行（见下）。
      */}
      <table className={cn("w-full min-w-0 border-collapse text-sm", fixed && "table-fixed")}>
        <thead className="sticky top-0 z-10">
          <tr>
            {selection ? (
              <th className="w-10 rounded-l-md bg-muted px-3 py-2">
                <input
                  type="checkbox"
                  checked={allSelected}
                  onChange={toggleAll}
                  aria-label="全选本页"
                  className="size-3.5 cursor-pointer accent-primary align-middle"
                />
              </th>
            ) : null}
            {columns.map((column, index) => (
              <th
                key={column.key}
                scope="col"
                style={column.width ? { width: column.width } : undefined}
                className={cn(
                  "bg-muted px-4 py-2 text-xs font-normal whitespace-nowrap text-muted-foreground",
                  !selection && index === 0 && "rounded-l-md",
                  index === columns.length - 1 && "rounded-r-md",
                  column.align === "left" && "text-left",
                  column.align === "right" && "text-right",
                  (column.align ?? "center") === "center" && "text-center",
                  column.hideOnMobile && "hidden md:table-cell",
                )}
              >
                {column.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => {
            const key = rowKey(row);
            const rowSelectable = selection?.isSelectable?.(row) ?? true;
            return (
              <tr
                key={key}
                onClick={onRowClick ? () => onRowClick(row) : undefined}
                tabIndex={onRowClick ? 0 : undefined}
                onKeyDown={
                  onRowClick
                    ? (event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          onRowClick(row);
                        }
                      }
                    : undefined
                }
                className={cn(
                  "border-b border-border transition-colors last:border-b-0",
                  striped && index % 2 === 1 && "bg-muted/40",
                  onRowClick
                    ? "cursor-pointer hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
                    : "hover:bg-muted/60",
                  selection?.selected.has(key) && "bg-primary/5",
                )}
              >
                {selection ? (
                  <td className="px-3 py-3">
                    <input
                      type="checkbox"
                      checked={selection.selected.has(key)}
                      disabled={!rowSelectable}
                      onChange={() => toggleOne(key)}
                      // 别让点复选框触发行点击（会被跳走）。
                      onClick={(event) => event.stopPropagation()}
                      aria-label={selection.label?.(row) ?? `选择 ${key}`}
                      className="size-3.5 cursor-pointer accent-primary align-middle disabled:cursor-not-allowed disabled:opacity-40"
                    />
                  </td>
                ) : null}
                {columns.map((column) => (
                  <td
                    key={column.key}
                    className={cn(
                      "px-4 py-3 align-middle",
                      // 单元格用 `bodyAlign`，没传才回落到 `align`。
                      // 表头始终按 `align`，两者可以不同（见类型定义里的说明）。
                      (column.bodyAlign ?? column.align ?? "center") === "left" && "text-left",
                      (column.bodyAlign ?? column.align ?? "center") === "right" &&
                        "text-right tabular-nums",
                      (column.bodyAlign ?? column.align ?? "center") === "center" && "text-center",
                      column.hideOnMobile && "hidden md:table-cell",
                    )}
                  >
                    {column.render(row)}
                  </td>
                ))}
              </tr>
            );
          })}

          {/*
            吸收剩余高度的占位行。
            `<table class="h-full">` 只保证表格整体有高度，多出来的空间会被
            均摊到各行上（把数据行拉高），而不是留在底部。
            这一行 `h-full` + 无边框，负责吃掉剩余空间，
            这样数据行保持 `py-3` 的自然高度，分隔线也不会悬在半空。
          */}
          <tr aria-hidden="true" className="h-full">
            <td colSpan={columns.length + (selection ? 1 : 0)} className="p-0" />
          </tr>
        </tbody>
      </table>
    </div>
  );
}
