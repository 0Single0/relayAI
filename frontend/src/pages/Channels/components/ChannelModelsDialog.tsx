import { useCallback, useEffect, useMemo, useState } from "react";
import { listChannelModels, updateChannel } from "@/api/channels";
import type { Channel } from "@/api/types";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { toast } from "@/components/common/Toast/store";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

type Props = { channel: Channel | null; onClose: () => void; onSaved: () => void };

export const ChannelModelsDialog = ({ channel, onClose, onSaved }: Props) => {
  const [models, setModels] = useState<string[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [publishAll, setPublishAll] = useState(true);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    if (!channel) return;
    let active = true;
    setSelected(channel.modelAllow ?? []);
    setPublishAll(!channel.modelAllow?.length);
    setModels([]);
    setError("");
    setLoadError("");
    setLoading(true);
    const load = async () => {
      try {
        const response = channel.accountCount > 0 ? await listChannelModels(channel.id) : null;
        if (active) setModels([...new Set((response?.data ?? []).map((model) => model.id))].sort());
      } catch (requestError) {
        if (active) setLoadError(getRequestErrorMessage(requestError));
      } finally {
        if (active) setLoading(false);
      }
    };
    void load();
    return () => { active = false; };
  }, [channel, reload]);

  const unsupported = useMemo(() => selected.filter((model) => !models.includes(model)), [models, selected]);

  const save = useCallback(async () => {
    if (!channel) return;
    if (!publishAll && (selected.length === 0 || unsupported.length > 0)) {
      setError(unsupported.length ? "请先移除当前账号已不支持的模型" : "请至少选择一个模型，或开启「发布全部支持模型」");
      return;
    }
    setPending(true);
    setError("");
    try {
      await updateChannel(channel.id, { modelAllow: publishAll ? [] : selected });
      toast.success("对外模型配置已保存");
      onSaved();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally { setPending(false); }
  }, [channel, onSaved, publishAll, selected, unsupported.length]);

  return <Dialog open={channel !== null} onOpenChange={(open) => { if (!open && !pending) onClose(); }}>
    <DialogContent className="max-w-xl" onInteractOutside={(event) => { if (pending) event.preventDefault(); }}>
      <DialogHeader>
        <DialogTitle>对外模型 · {channel?.name}</DialogTitle>
        <DialogDescription>仅发布本分组已启用账号支持的模型。分组没有账号时不提供任何模型。</DialogDescription>
      </DialogHeader>
      <MyBox isLoading={loading} text="正在加载分组支持的模型..." className="min-h-24">
        {loadError ? <ErrorState message={loadError} onRetry={() => setReload((value) => value + 1)} /> :
          <div className="flex min-w-0 flex-col gap-3">
            <label className="flex items-center justify-between gap-3 text-sm">
              发布全部支持模型
              <Switch checked={publishAll} onCheckedChange={(checked) => { setPublishAll(checked); setError(""); }} disabled={pending} aria-label="发布全部支持模型" />
            </label>
            <p className="text-xs text-muted-foreground">{publishAll ? `当前支持 ${models.length} 个模型，账号能力变化后自动更新。` : `当前选择 ${selected.length} 个模型。`}</p>
            {models.length === 0 ? <p className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">暂无可发布模型。请先添加并启用同渠道账号；自定义供应商账号需要配置支持模型。</p> :
              <div className="flex max-h-64 flex-wrap gap-2 overflow-auto rounded-md border p-3">
                {models.map((model) => {
                  const checked = publishAll || selected.includes(model);
                  return <button key={model} type="button" aria-pressed={checked} disabled={pending || publishAll}
                    onClick={() => { setSelected((previous) => previous.includes(model) ? previous.filter((item) => item !== model) : [...previous, model]); setError(""); }}
                    className={cn("max-w-full rounded-md border px-2 py-1 text-left font-mono text-xs break-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", checked ? "border-primary/40 bg-primary/10 text-primary" : "text-muted-foreground hover:bg-muted")}>
                    {model}
                  </button>;
                })}
              </div>}
            {!publishAll && unsupported.length > 0 ? <div className="text-xs text-muted-foreground">
              <p>以下已选模型当前不受支持，请移除后保存：</p>
              {unsupported.map((model) => <Button key={model} size="sm" variant="ghost" disabled={pending} className="max-w-full whitespace-normal break-all" onClick={() => { setSelected((previous) => previous.filter((item) => item !== model)); setError(""); }}>移除 {model}</Button>)}
            </div> : null}
          </div>}
      </MyBox>
      {error ? <ErrorState title="保存失败" message={error} /> : null}
      <DialogFooter>
        <Button size="sm" variant="outline" onClick={onClose} disabled={pending}>取消</Button>
        <Button size="sm" onClick={() => void save()} disabled={pending || loading || !!loadError}>{pending ? "保存中..." : "保存"}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
};
