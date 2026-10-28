import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";

const PAGE_SIZES = [20, 50, 100] as const;

export type PaginationProps = {
  /** 当前页，从 1 开始。 */
  page: number;
  pageSize: number;
  total: number;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
  disabled?: boolean;
  className?: string;
};

/**
 * 服务端分页条：**固定在右下角**。
 *
 * 为什么放右下角而不是居中或左上：
 *   · 表格从左上往下读，操作完一行时视线已经在右侧；
 *   · 右下角是"翻页"这个动作在几乎所有后台里的位置，用户不用找。
 *
 * 为什么必须是服务端分页：`usage_logs` 是十万量级，前端分页意味着
 * 先把全量拉到浏览器（几十 MB）。这里所有列表页都走 `page` / `pageSize` 参数。
 *
 * 页码显示 `第 N / M 页`，不做长页码列表：后台翻页几乎只用"上一页 / 下一页"，
 * 一串页码反而占地方、还要处理省略号逻辑。
 */
export const Pagination = ({
  page,
  pageSize,
  total,
  onPageChange,
  onPageSizeChange,
  disabled,
  className,
}: PaginationProps) => {
  const totalPages = Math.max(1, Math.ceil(total / Math.max(pageSize, 1)));

  return (
    <div
      className={cn(
        "flex shrink-0 flex-wrap items-center justify-end gap-3 text-xs text-muted-foreground",
        className,
      )}
    >
      <span className="tabular-nums">
        共 {total.toLocaleString("zh-CN")} 条 · 第 {page} / {totalPages} 页
      </span>

      <div className="w-[5.5rem]">
        <Select
          value={String(pageSize)}
          onValueChange={(value) => onPageSizeChange(Number(value))}
        >
          <SelectTrigger aria-label="每页条数" className="h-7 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {PAGE_SIZES.map((size) => (
              <SelectItem key={size} value={String(size)}>
                每页 {size} 条
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="flex items-center gap-1">
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="上一页"
          title="上一页"
          disabled={disabled || page <= 1}
          onClick={() => onPageChange(Math.max(1, page - 1))}
        >
          <ChevronLeft aria-hidden="true" />
        </Button>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="下一页"
          title="下一页"
          disabled={disabled || page >= totalPages}
          onClick={() => onPageChange(Math.min(totalPages, page + 1))}
        >
          <ChevronRight aria-hidden="true" />
        </Button>
      </div>
    </div>
  );
};
