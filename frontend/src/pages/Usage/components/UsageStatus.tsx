import { Info } from "lucide-react";
import { Popover } from "radix-ui";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { UsageLog, UsageStatus as UsageStatusValue } from "@/api/types";
import { StatusBadge } from "@/components/common/StatusBadge";

const statusTone = (status: UsageStatusValue) => {
  switch (status) {
    case "completed": return "success" as const;
    case "rejected": return "neutral" as const;
    case "client_aborted": return "warning" as const;
    default: return "danger" as const;
  }
};

const statusLabel = (status: UsageStatusValue) => {
  switch (status) {
    case "completed": return "成功";
    case "failed": return "失败";
    case "client_aborted": return "客户端中断";
    case "upstream_aborted": return "上游断流";
    case "rejected": return "未调度";
    default: return status;
  }
};

/** 只格式化合法 JSON；HTML 等响应始终作为文本展示。 */
const formatBody = (body: string) => {
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
};

const splitDetail = (detail: string | null) => {
  const text = detail?.trim() || "这条记录未保存错误详情，无法还原当时的上游响应。";
  const marker = "；上游返回：";
  const index = text.indexOf(marker);
  return index < 0
    ? { reason: text, upstream: null }
    : { reason: text.slice(0, index), upstream: formatBody(text.slice(index + marker.length)) };
};

/** Portal 避免被表格滚动容器裁掉；悬浮、键盘聚焦和触屏点击均可查看。 */
export const UsageStatus = ({ log }: { log: UsageLog }) => {
  const [open, setOpen] = useState(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const titleId = useId();

  const cancelClose = useCallback(() => {
    if (closeTimer.current !== null) clearTimeout(closeTimer.current);
    closeTimer.current = null;
  }, []);
  useEffect(() => cancelClose, [cancelClose]);

  const show = useCallback(() => {
    cancelClose();
    setOpen(true);
  }, [cancelClose]);
  const scheduleClose = useCallback(() => {
    cancelClose();
    closeTimer.current = setTimeout(() => {
      // 点击/键盘打开后可继续选取文字，Tab 移出由 Radix 关闭。
      if (document.activeElement !== trigger.current && !panel.current?.contains(document.activeElement)) {
        setOpen(false);
      }
    }, 180);
  }, [cancelClose]);

  const badge = <StatusBadge tone={statusTone(log.status)} label={statusLabel(log.status)} />;
  if (log.status === "completed") return badge;
  const detail = splitDetail(log.errorDetail);

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          ref={trigger}
          type="button"
          aria-label={`${statusLabel(log.status)}，查看错误详情`}
          className="inline-flex cursor-help items-center gap-1 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
          onMouseEnter={show}
          onMouseLeave={scheduleClose}
          onFocus={show}
          onClick={(event) => { event.preventDefault(); show(); }}
        >
          {badge}
          <Info className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          ref={panel}
          aria-labelledby={titleId}
          side="bottom"
          align="end"
          sideOffset={8}
          collisionPadding={12}
          onMouseEnter={cancelClose}
          onMouseLeave={scheduleClose}
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
          onEscapeKeyDown={() => trigger.current?.focus()}
          className="z-50 flex max-h-[min(32rem,var(--radix-popover-content-available-height))] w-[min(30rem,calc(100vw-1.5rem))] flex-col overflow-hidden rounded-lg border bg-popover text-left text-popover-foreground shadow-sm outline-none"
        >
          <div className="shrink-0 border-b px-4 py-3">
            <p id={titleId} className="text-sm font-medium">请求错误详情</p>
            <p className="mt-1 break-all text-xs text-muted-foreground">请求 ID：{log.requestId || "未记录"}</p>
          </div>
          <div tabIndex={0} aria-label="错误内容，可滚动查看" className="min-h-0 overflow-auto p-4 text-xs leading-5 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
            <p className="mb-1 font-medium">失败原因</p>
            <p className="whitespace-pre-wrap [overflow-wrap:anywhere]">{detail.reason}</p>
            {detail.upstream !== null ? (
              <>
                <p className="mt-4 mb-1 font-medium">上游返回</p>
                <pre className="whitespace-pre-wrap font-mono text-[11px] leading-5 text-muted-foreground [overflow-wrap:anywhere]">{detail.upstream}</pre>
              </>
            ) : null}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
};
