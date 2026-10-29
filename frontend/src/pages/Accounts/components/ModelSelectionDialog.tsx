import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { CustomModelDefinition } from "@/api/accounts/models";
import { MyBox } from "@/components/common/MyBox";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";

type Props = {
  trigger: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  models: CustomModelDefinition[];
  existing: string[];
  fetching: boolean;
  error: string;
  onRetry: () => void;
  onConfirm: (models: CustomModelDefinition[]) => void;
};

export const ModelSelectionDialog = ({ trigger, open, onOpenChange, models, existing, fetching, error, onRetry, onConfirm }: Props) => {
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const searchInput = useRef<HTMLInputElement>(null);
  useEffect(() => { if (open) { setSearch(""); setSelected(new Set()); } }, [open, models]);
  useEffect(() => { if (open && !fetching) searchInput.current?.focus({ preventScroll: true }); }, [open, fetching]);
  const existingIDs = useMemo(() => new Set(existing), [existing]);
  const filtered = useMemo(() => models.filter(model => `${model.id} ${model.display_name ?? ""}`.toLowerCase().includes(search.trim().toLowerCase())), [models, search]);
  const selectable = useMemo(() => filtered.filter(model => !existingIDs.has(model.id)), [existingIDs, filtered]);
  const allSelected = selectable.length > 0 && selectable.every(model => selected.has(model.id));

  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogTrigger asChild>{trigger}</DialogTrigger>
    <DialogContent className="flex max-h-[calc(100dvh-3rem)] max-w-lg flex-col overflow-hidden">
      <DialogHeader><DialogTitle>选择要添加的模型</DialogTitle><DialogDescription>以下是模型提供商的可用模型，勾选后点击「添加所选」。已有模型的配置会保留。</DialogDescription></DialogHeader>
      <div className="flex min-w-0 items-center gap-2">
        <input ref={searchInput} aria-label="搜索模型" value={search} onChange={event => setSearch(event.target.value)} disabled={fetching} placeholder="搜索模型" className="h-9 min-w-0 flex-1 rounded-md border border-input bg-background px-3 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/30" />
        <Button type="button" variant="ghost" size="sm" disabled={fetching || !selectable.length} onClick={() => setSelected(current => { const next = new Set(current); for (const model of selectable) { if (allSelected) next.delete(model.id); else next.add(model.id); } return next; })}>{allSelected ? "取消全选" : "全选"}</Button>
      </div>
      <MyBox isLoading={fetching} text="正在获取模型列表..." className="flex min-h-36 min-w-0 flex-1 flex-col overflow-hidden">
        {error ? <div className="flex min-h-36 flex-col items-center justify-center gap-3 px-2 py-4"><p role="alert" className="text-xs leading-5 break-words text-destructive">{error}</p><Button type="button" variant="outline" size="sm" onClick={onRetry}>重试获取</Button></div> : <div className="max-h-[45dvh] min-h-36 overflow-auto" role="group" aria-label="可用模型">
          {filtered.length ? filtered.map(model => {
            const added = existingIDs.has(model.id);
            return <label key={model.id} className="flex min-w-0 cursor-pointer items-start gap-2.5 rounded-md px-2 py-2.5 text-sm hover:bg-muted/60 focus-within:ring-2 focus-within:ring-ring focus-within:ring-inset">
              <input type="checkbox" aria-label={`选择模型 ${model.id}`} className="mt-0.5 size-4 shrink-0 accent-primary" disabled={added || fetching} checked={added || selected.has(model.id)} onChange={event => setSelected(current => { const next = new Set(current); if (event.target.checked) next.add(model.id); else next.delete(model.id); return next; })} />
              <span className="min-w-0 flex-1 break-all"><span className="font-mono text-xs">{model.id}</span>{model.display_name && model.display_name !== model.id ? <span className="mt-0.5 block text-xs text-muted-foreground">{model.display_name}</span> : null}</span>
              {added ? <span className="shrink-0 text-[11px] text-muted-foreground">已添加</span> : null}
            </label>;
          }) : <p className="px-2 py-6 text-center text-xs text-muted-foreground">{models.length ? "没有匹配的模型" : fetching ? "正在获取模型列表..." : "上游未返回模型，可取消后手动添加。"}</p>}
        </div>}
      </MyBox>
      <div className="text-xs text-muted-foreground" role="status">共 {models.length} 个模型，已选择 {selected.size} 个待添加模型</div>
      <DialogFooter><Button type="button" variant="outline" onClick={() => onOpenChange(false)}>取消</Button><Button type="button" disabled={fetching || Boolean(error) || !selected.size} onClick={() => onConfirm(models.filter(model => selected.has(model.id)))}>添加所选{selected.size ? `（${selected.size}）` : ""}</Button></DialogFooter>
    </DialogContent>
  </Dialog>;
};
