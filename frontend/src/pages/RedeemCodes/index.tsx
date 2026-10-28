import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, Copy, Plus, RefreshCw, Ticket } from "lucide-react";
import { createRedeemCodes, listRedeemCodes, type RedeemCode } from "@/api/auth";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const formatMoney = (value: number) => `¥${value.toFixed(2)}`;

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm tabular-nums",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

/**
 * 兑换码管理（仅管理员）。
 *
 * 独立成页而不是和用户管理挤在一页：这一页的动作是"批量发码、批量复制、
 * 看谁还没用"，与"改某个人余额"是完全不同的两种工作。
 * 放在一起时两边的表格都要让出一半高度，谁都看不全。
 */
export const RedeemCodesPage = () => {
  const [codes, setCodes] = useState<RedeemCode[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [copied, setCopied] = useState("");

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listRedeemCodes({ pageSize: 200 });
      setCodes(response.data?.list ?? []);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const stats = useMemo(() => {
    const unused = codes.filter((code) => code.usedAt === null);
    const usedCount = codes.length - unused.length;
    const unusedAmount = unused.reduce((sum, code) => sum + code.amount, 0);
    return { unused: unused.length, usedCount, unusedAmount };
  }, [codes]);

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(text);
      // 2 秒后复位图标：一直显示"已复制"会让人分不清哪一个刚复制。
      window.setTimeout(() => setCopied(""), 2000);
    } catch {
      // 剪贴板被浏览器拒绝（非安全上下文）时选中文本作为退路。
      setCopied("");
    }
  };

  const copyAllUnused = () => {
    const text = codes
      .filter((code) => code.usedAt === null)
      .map((code) => code.code)
      .join("\n");
    if (text) {
      void copy(text);
    }
  };

  const columns: Array<Column<RedeemCode>> = [
    {
      key: "code",
      header: "兑换码",
      render: (code) => (
        <div className="flex items-center gap-1.5">
          <span className="font-mono text-xs">{code.code}</span>
          <Button
            variant="ghost"
            size="icon-xs"
            onClick={() => void copy(code.code)}
            aria-label={`复制 ${code.code}`}
            title="复制"
          >
            {copied === code.code ? (
              <Check className="text-success" aria-hidden="true" />
            ) : (
              <Copy aria-hidden="true" />
            )}
          </Button>
        </div>
      ),
    },
    {
      key: "amount",
      header: "面额",
      width: "100px",
      align: "center",
      render: (code) => <span className="tabular-nums">{formatMoney(code.amount)}</span>,
    },
    {
      key: "status",
      header: "状态",
      width: "110px",
      render: (code) => (
        <StatusBadge
          tone={code.usedAt ? "neutral" : "success"}
          label={code.usedAt ? "已使用" : "未使用"}
        />
      ),
    },
    {
      key: "note",
      header: "备注",
      hideOnMobile: true,
      render: (code) => (
        <span className="truncate text-xs text-muted-foreground">{code.note || "—"}</span>
      ),
    },
    {
      key: "created",
      header: "生成时间",
      width: "160px",
      hideOnMobile: true,
      render: (code) => (
        <span className="text-xs tabular-nums text-muted-foreground">
          {new Date(code.createdAt).toLocaleString("zh-CN")}
        </span>
      ),
    },
  ];

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">
          共 {codes.length} 个 · 未使用{" "}
          <span className="font-medium text-success tabular-nums">{stats.unused}</span> 个（面额{" "}
          {formatMoney(stats.unusedAmount)}）· 已使用 {stats.usedCount} 个
        </span>

        <div className="ml-auto flex items-center gap-2">
          {stats.unused > 0 ? (
            <Button variant="outline" size="sm" onClick={copyAllUnused}>
              <Copy aria-hidden="true" />
              复制未使用
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
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus aria-hidden="true" />
            生成兑换码
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox
        isLoading={isLoading && codes.length === 0}
        text="正在加载兑换码..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={codes}
          rowKey={(code) => String(code.id)}
          empty={
            <EmptyState
              icon={Ticket}
              title="还没有兑换码"
              description="批量生成后发给用户，他们可以在「用户中心」自助充值。"
              action={
                <Button size="sm" onClick={() => setCreateOpen(true)}>
                  <Plus aria-hidden="true" />
                  生成兑换码
                </Button>
              }
            />
          }
        />
      </MyBox>

      <CreateCodesDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={() => setReloadKey((value) => value + 1)}
      />
    </div>
  );
};

/** 批量生成兑换码。 */
const CreateCodesDialog = ({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
}) => {
  const [amount, setAmount] = useState("100");
  const [count, setCount] = useState("5");
  const [note, setNote] = useState("");
  const [pending, setPending] = useState(false);
  const [formError, setFormError] = useState("");
  const [created, setCreated] = useState<RedeemCode[]>([]);

  useEffect(() => {
    if (!open) {
      setCreated([]);
      setFormError("");
      setAmount("100");
      setCount("5");
      setNote("");
    }
  }, [open]);

  const submit = async () => {
    const parsedAmount = Number(amount);
    const parsedCount = Number(count);
    if (!Number.isFinite(parsedAmount) || parsedAmount <= 0) {
      setFormError("面额必须大于 0");
      return;
    }
    if (!Number.isInteger(parsedCount) || parsedCount < 1 || parsedCount > 500) {
      setFormError("数量需为 1–500 的整数");
      return;
    }

    setPending(true);
    setFormError("");
    try {
      const response = await createRedeemCodes({
        amount: parsedAmount,
        count: parsedCount,
        note: note.trim(),
      });
      setCreated(response.data ?? []);
      onCreated();
    } catch (requestError) {
      setFormError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  const createdText = created.map((code) => code.code).join("\n");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>生成兑换码</DialogTitle>
          <DialogDescription>
            批量生成一次性充值码。每个码只能被使用一次，已排除容易看错的 0/O/1/I。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">面额（元）</label>
              <input
                value={amount}
                onChange={(event) => setAmount(event.target.value)}
                inputMode="decimal"
                className={inputClass}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">数量</label>
              <input
                value={count}
                onChange={(event) => setCount(event.target.value)}
                inputMode="numeric"
                className={inputClass}
              />
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">备注（可选）</label>
            <input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="例如：十月推广"
              className={inputClass}
            />
          </div>

          {formError ? <ErrorState title="生成失败" message={formError} /> : null}

          {created.length > 0 ? (
            <div className="flex flex-col gap-1.5">
              <div className="flex items-center gap-2">
                <p className="text-xs font-medium text-success">
                  已生成 {created.length} 个：
                </p>
                <Button
                  variant="ghost"
                  size="xs"
                  onClick={() => void navigator.clipboard.writeText(createdText)}
                >
                  <Copy aria-hidden="true" />
                  全部复制
                </Button>
              </div>
              <div className="max-h-40 overflow-auto rounded-md border bg-muted/30 p-2">
                <ul className="flex flex-col gap-1">
                  {created.map((code) => (
                    <li key={code.id} className="font-mono text-xs">
                      {code.code}
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending}>
            {pending ? "生成中..." : "生成"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
