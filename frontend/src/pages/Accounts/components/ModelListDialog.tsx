import { useMemo, useState } from "react";
import { ChevronDown } from "lucide-react";
import type { Account } from "@/api/types";
import type { CatalogModel } from "@/api/catalog";
import { AccountModelCard } from "./AccountModelCard";
import { WorkbuddyModelList } from "./WorkbuddyModelList";
import {
  activeLimits,
  usableModels,
} from "@/pages/Accounts/components/modelCaps";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/**
 * 模型清单弹窗。
 *
 * 为什么用弹窗而不是行内展开：
 *   一个账号十几到几十个模型，行内展开会把行高从 60px 撑到几百像素，
 *   相邻行被挤出屏幕 —— 一眼看整张表的能力就没了。
 *   弹窗只覆盖不改变布局，还能放下每个模型的完整规格。
 *
 * 卡片形态对齐参考实现：模型名 + 限流倒计时 + 规格 + 能力徽标，
 * 一眼能判断"这个模型现在能不能用、适合什么活"。
 */

/** 「模型」列里的按钮：显示数量，点击开弹窗。 */
export const ModelListButton = ({
  account,
  catalog,
  onOpen,
}: {
  account: Account;
  catalog: CatalogModel[];
  onOpen: () => void;
}) => {
  const [now] = useState(() => Date.now());
  const models = useMemo(() => usableModels(account, catalog), [account, catalog]);
  const limits = activeLimits(account, now);
  const isWorkbuddy = (account.platform || account.adapter) === "workbuddy";

  if (models.length === 0 && !isWorkbuddy) {
    return (
      <span
        className="text-xs text-muted-foreground"
        title="尚无账号支持的模型；自定义供应商可在编辑账号中添加模型"
      >
        暂无模型
      </span>
    );
  }

  return (
    <button
      type="button"
      onClick={onOpen}
      title={isWorkbuddy ? "实时查询账号支持的模型与官方积分规则" : "查看模型清单与限流情况"}
      className={cn(
        "inline-flex cursor-pointer items-center gap-1 text-xs transition-colors",
        limits.length > 0 ? "text-warning hover:text-warning/80" : "hover:text-foreground",
        "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
      )}
    >
      <span>
        {isWorkbuddy ? "查看模型" : `${models.length} 个`}
        {limits.length > 0 ? ` · ${limits.length} 限流` : ""}
      </span>
      {limits.length > 0 ? (
        <span className="size-1.5 rounded-full bg-warning" aria-hidden="true" />
      ) : null}
      <ChevronDown className="size-3" aria-hidden="true" />
    </button>
  );
};

/** 模型清单弹窗本体。 */
export const ModelListDialog = ({
  account,
  catalog,
  onClose,
}: {
  /** null 表示关闭。 */
  account: Account | null;
  catalog: CatalogModel[];
  onClose: () => void;
}) => {
  const [now] = useState(() => Date.now());
  const models = useMemo(
    () => (account ? usableModels(account, catalog) : []),
    [account, catalog],
  );
  const limits = account ? activeLimits(account, now) : [];
  const limitedCount = limits.length;
  const isWorkbuddy = account !== null && (account.platform || account.adapter) === "workbuddy";

  return (
    <Dialog open={account !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>账号支持的模型</DialogTitle>
          <DialogDescription>
            {account
              ? isWorkbuddy ? `${account.nickname || account.uid} · 官方积分规则` : `${account.nickname || account.uid} · 共 ${models.length} 个${
                  limitedCount > 0 ? `，其中 ${limitedCount} 个正被上游限流` : ""
                }`
              : ""}
          </DialogDescription>
        </DialogHeader>
        <p className="text-xs text-muted-foreground">这里展示账号自身支持的模型；对外调用还需分配分组，并由分组开放对应模型。</p>

        {!isWorkbuddy && limitedCount > 0 ? (
          <p className="rounded-md border border-warning/30 bg-warning/5 px-2.5 py-1.5 text-[11px] text-warning">
            上游按模型限额：标黄的那些暂时不可用，其余模型照常服务 —— 账号本身没问题。
          </p>
        ) : null}

        {isWorkbuddy && account ? <WorkbuddyModelList key={account.id} account={account} /> : models.length === 0 ? (
          <p className="py-6 text-center text-xs text-muted-foreground">
            尚未获取到账号模型。自定义供应商可在编辑账号中添加或获取模型列表。
          </p>
        ) : (
          <div className="grid max-h-[60vh] grid-cols-[repeat(auto-fill,minmax(min(100%,240px),1fr))] gap-2 overflow-y-auto">
            {models.map((model) => {
              const limit = limits.find((item) => item.model === model.name);
              return (
                <AccountModelCard
                  key={model.name}
                  model={model}
                  untilMs={limit?.untilMs}
                  now={now}
                />
              );
            })}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
};
