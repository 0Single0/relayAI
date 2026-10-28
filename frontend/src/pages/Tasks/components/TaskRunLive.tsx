import { Radio } from "lucide-react";
import { useEffect } from "react";
import type { TaskRunItem } from "@/api/types";
import { StatusBadge, type StatusTone } from "@/components/common/StatusBadge";
import { MyBox } from "@/components/common/MyBox";
import { useTaskStream } from "@/hooks/useTaskStream";

type TaskRunLiveProps = {
  runId: number;
  /** 执行结束后通知外层刷新一次快照（积分、进度都会变）。 */
  onFinished: () => void;
};

const itemTone = (status: string): StatusTone => {
  switch (status) {
    case "done":
      return "success";
    case "failed":
      return "danger";
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
    default:
      return status;
  }
};

const kindLabel = (kind: TaskRunItem["kind"]) => {
  switch (kind) {
    case "daily":
      return "每日";
    case "checkin":
      return "签到";
    default:
      return "任务";
  }
};

/**
 * 本次执行的逐条实时结果（SPEC 9.8「行内实时结果通过 SSE 更新」）。
 *
 * 与上方"任务清单"的区别：清单是**上游状态**（哪些达标、哪些早已领过），
 * 这里是**这一次执行发生了什么**（哪一项跳过、跳过原因是什么）。
 * 两者不能互相替代 —— 清单告诉你"还差什么"，这里告诉你"刚才做了什么"。
 */
export const TaskRunLive = ({ runId, onFinished }: TaskRunLiveProps) => {
  const { items, progress, finished, error } = useTaskStream(runId);

  // 执行结束：刷新一次外层快照。用 effect 而不是在帧回调里直接调，
  // 是为了避免 `finished` 因重连等原因翻转时重复刷新。
  useEffect(() => {
    if (finished) {
      onFinished();
    }
  }, [finished, onFinished]);

  const done = progress?.doneCount ?? 0;
  const total = progress?.total ?? 0;

  return (
    <div className="mb-3">
      <div className="mb-1 flex flex-wrap items-center gap-2">
        <p className="text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
          本次执行
        </p>
        {error ? (
          <StatusBadge tone="warning" label={error} dot={false} />
        ) : finished ? (
          <StatusBadge tone="success" label="已结束" />
        ) : (
          <StatusBadge tone="info" label={`执行中 ${done}/${total || "?"}`} />
        )}
        {progress?.earned ? (
          <span className="text-[11px] tabular-nums text-success">
            +{progress.earned} 积分
          </span>
        ) : null}
      </div>

      <MyBox
        isLoading={items.length === 0 && !finished && !error}
        text="正在等待第一条结果..."
      >
        {items.length === 0 ? (
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Radio className="size-3.5" aria-hidden="true" />
            {error ? "没有收到结果" : "正在执行，结果会逐条出现在这里。"}
          </p>
        ) : (
          <ul className="space-y-1">
            {items.map((item) => (
              <li
                key={item.id}
                className="flex flex-wrap items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 text-xs"
              >
                <StatusBadge tone="neutral" label={kindLabel(item.kind)} dot={false} />
                <span className="min-w-0 flex-1 truncate font-medium">{item.name}</span>
                {item.earned > 0 ? (
                  <span className="shrink-0 tabular-nums text-success">
                    +{item.earned}
                  </span>
                ) : null}
                <StatusBadge
                  tone={itemTone(item.status)}
                  label={itemLabel(item.status)}
                />
                {item.message ? (
                  <span className="min-w-0 basis-full break-words text-[11px] text-muted-foreground">
                    {item.message}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </MyBox>
    </div>
  );
};
