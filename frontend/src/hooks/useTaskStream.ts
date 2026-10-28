import { useEffect, useRef, useState } from "react";
import {
  type TaskStreamProgress,
  taskStreamUrl,
} from "@/api/tasks";
import type { TaskRunItem } from "@/api/types";

/**
 * 订阅一次任务执行的 SSE 进度（SPEC 9.8）。
 *
 * 为什么用 `EventSource` 而不是轮询：执行是一秒一条的节奏，
 * 轮询要么太慢（看不出"正在做哪一项"），要么把接口打成每秒 N 次。
 *
 * `runId` 为 `null` 时不开连接。连接由浏览器自己重连（`EventSource`
 * 规范行为），断点通过 `after` 参数传回去，所以重连不会重放历史帧。
 *
 * 返回的 `items` 是**增量累积**的：每收到一条 `item` 帧就追加。
 * 切到别的账号时 `runId` 变化，effect 重建，累积自然清空。
 */
export const useTaskStream = (runId: number | null) => {
  const [items, setItems] = useState<TaskRunItem[]>([]);
  const [progress, setProgress] = useState<TaskStreamProgress | null>(null);
  const [finished, setFinished] = useState(false);
  const [error, setError] = useState("");

  // 已收到的最后一条明细 id。重连时带回去当断点。
  const lastItemId = useRef(0);
  const seen = useRef<Set<number>>(new Set());

  useEffect(() => {
    setItems([]);
    setProgress(null);
    setFinished(false);
    setError("");
    lastItemId.current = 0;
    seen.current = new Set();

    if (runId === null) {
      return;
    }

    let source: EventSource | null = null;
    let closed = false;

    const connect = () => {
      if (closed) {
        return;
      }
      source = new EventSource(taskStreamUrl(runId, lastItemId.current));

      source.addEventListener("item", (event) => {
        const item = JSON.parse((event as MessageEvent<string>).data) as TaskRunItem;
        // 断点重连时服务端可能重发边界上的那一条，按 id 去重。
        if (seen.current.has(item.id)) {
          return;
        }
        seen.current.add(item.id);
        lastItemId.current = Math.max(lastItemId.current, item.id);
        setItems((prev) => [...prev, item]);
      });

      source.addEventListener("progress", (event) => {
        setProgress(
          JSON.parse((event as MessageEvent<string>).data) as TaskStreamProgress,
        );
      });

      source.addEventListener("done", () => {
        setFinished(true);
        source?.close();
      });

      source.addEventListener("failed", (event) => {
        const payload = JSON.parse(
          (event as MessageEvent<string>).data,
        ) as { message?: string };
        setError(payload.message || "进度连接失败");
        source?.close();
      });

      source.addEventListener("error", () => {
        // 连接层错误：`EventSource` 会自己重连（`readyState === CONNECTING`）。
        // 只有被彻底关闭时才需要用户知道。
        if (source?.readyState === EventSource.CLOSED) {
          setError("进度连接已断开，刷新可查看最新结果");
        }
      });
    };

    connect();

    return () => {
      closed = true;
      source?.close();
    };
  }, [runId]);

  return { items, progress, finished, error };
};
