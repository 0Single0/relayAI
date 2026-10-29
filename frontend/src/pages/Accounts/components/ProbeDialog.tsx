import { useEffect, useMemo, useState } from "react";
import { CircleAlert, CircleCheck, LoaderCircle, Play } from "lucide-react";
import type { Account, Model } from "@/api/types";
import { probeAccount } from "@/api/accounts";
import { listModels } from "@/api/channels";
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
import { toast } from "@/components/common/Toast/store";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

/**
 * 测试凭据：选一个模型，真实发一句 hi，结果直接显示在弹窗里。
 *
 * 为什么要能选模型：
 *   上游的限流与权限**按模型**生效。号在 `glm-5.3` 上被限流、
 *   但 `deepseek-v4.1-flash` 完全可用 —— 只测"默认模型"会得到
 *   一个以偏概全的结论，反而误导人以为号废了。
 *
 * 为什么结果放弹窗而不是 toast：
 *   失败时上游会回一大段 JSON（错误码、requestId、恢复时刻），
 *   toast 放不下也留不住。弹窗能完整展示，还方便复制。
 */

/** 测试结果：成功或失败都保留完整原文。 */
type ProbeResult = {
  ok: boolean;
  latencyMs: number;
  reply: string;
  error: string | null;
};

export const ProbeDialog = ({
  account,
  onClose,
}: {
  /** 要测试的账号；null 表示弹窗关闭。 */
  account: Account | null;
  onClose: () => void;
}) => {
  const [models, setModels] = useState<Model[]>([]);
  const [model, setModel] = useState("");
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<ProbeResult | null>(null);

  // 打开时拉一次模型目录；空列表也要能测（网关会自动挑）。
  useEffect(() => {
    if (!account) {
      return;
    }
    setResult(null);
    setPending(false);
    setModel("");
    void listModels()
      .then((response) => setModels(response.data ?? []))
      .catch(() => setModels([]));
  }, [account]);

  /**
   * 可选的模型。
   *
   * 账号级白名单非空时只列那些 —— 测一个这号本来就不允许的模型，
   * 失败原因是"没权限"而不是"限流"，对排查没有帮助。
   */
  const options = useMemo(() => {
    const allow = account?.modelAllow ?? [];
    if (allow.length === 0) {
      return models;
    }
    const allowed = new Set(allow);
    return models.filter((item) => allowed.has(item.canonicalName));
  }, [models, account]);

  const run = async () => {
    if (!account) {
      return;
    }
    setPending(true);
    setResult(null);
    try {
      const response = await probeAccount(account.id, model || undefined);
      const data = response.data;
      setResult({
        ok: Boolean(data?.ok),
        latencyMs: data?.latencyMs ?? 0,
        reply: data?.reply ?? "",
        error: data?.error ?? null,
      });
    } catch (requestError) {
      // 请求本身失败（网关不可达 / 账号不存在）也如实展示，
      // 不要当成"测试没通过" —— 那会把基础设施问题误报成凭据问题。
      const message = getRequestErrorMessage(requestError);
      setResult({ ok: false, latencyMs: 0, reply: "", error: message });
      toast.error("测试请求失败", message);
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={account !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>测试凭据</DialogTitle>
          <DialogDescription>
            {account
              ? `${account.nickname || account.uid} · 会真实消耗一次上游额度`
              : ""}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium">测试模型</span>
            <Select value={model || "__auto__"} onValueChange={(next) => setModel(next === "__auto__" ? "" : next)}>
              <SelectTrigger aria-label="测试模型">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__auto__">自动（由网关挑一个可用模型）</SelectItem>
                {options.map((item) => (
                  <SelectItem key={item.canonicalName} value={item.canonicalName}>
                    {item.canonicalName}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <span className="text-[11px] text-muted-foreground">
              上游限流按模型生效。某个模型被限流时，换一个模型测才能判断号本身是否可用。
            </span>
          </div>

          {result ? (
            <div
              className={cn(
                "flex flex-col gap-1.5 rounded-md border px-3 py-2",
                result.ok
                  ? "border-success/40 bg-success/5"
                  : "border-destructive/40 bg-destructive/5",
              )}
            >
              <span
                className={cn(
                  "flex items-center gap-1.5 text-sm font-medium",
                  result.ok ? "text-success" : "text-destructive",
                )}
              >
                {result.ok ? (
                  <CircleCheck className="size-4" aria-hidden="true" />
                ) : (
                  <CircleAlert className="size-4" aria-hidden="true" />
                )}
                {result.ok ? `通过 · ${result.latencyMs}ms` : "未通过"}
              </span>
              {result.ok ? (
                <p className="break-words text-xs text-muted-foreground">
                  上游回显：{result.reply || "（空）"}
                </p>
              ) : (
                <pre className="max-h-48 overflow-auto break-words whitespace-pre-wrap text-[11px] text-muted-foreground">
                  {result.error || "上游没有返回可用响应"}
                </pre>
              )}
            </div>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} className="cursor-pointer">
            关闭
          </Button>
          <Button onClick={() => void run()} disabled={pending} className="cursor-pointer">
            {pending ? (
              <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
            ) : (
              <Play aria-hidden="true" />
            )}
            {pending ? "测试中..." : "开始测试"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
