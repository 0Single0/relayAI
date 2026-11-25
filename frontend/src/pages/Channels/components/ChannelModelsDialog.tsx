import { useCallback, useEffect, useMemo, useState } from "react";
import { RefreshCw, X } from "lucide-react";
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
import { matchesModelRule, recommendedModelRules } from "../modelRules";

type Props = { channel: Channel | null; onClose: () => void; onSaved: () => void };

export const ChannelModelsDialog = ({ channel, onClose, onSaved }: Props) => {
  const [models, setModels] = useState<string[]>([]);
  const [rules, setRules] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [publishAll, setPublishAll] = useState(true);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [error, setError] = useState("");
  const [pending, setPending] = useState(false);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    if (!channel) return;
    setRules(channel.modelAllow ?? []);
    setPublishAll(!channel.modelAllow?.length);
    setDraft("");
    setError("");
  }, [channel]);

  useEffect(() => {
    if (!channel) return;
    let active = true;
    setModels([]);
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

  const preview = useMemo(() => models.map((name) => ({
    name, matched: publishAll || rules.some((rule) => matchesModelRule(rule, name)),
  })), [models, publishAll, rules]);
  const waiting = useMemo(() => rules.filter((rule) => !models.some((name) => matchesModelRule(rule, name))), [models, rules]);
  const recommendations = useMemo(() => recommendedModelRules(channel?.modelProvider ?? ""), [channel?.modelProvider]);

  const addRule = useCallback((value: string) => {
    const rule = value.trim();
    if (!rule || /\s/.test(rule) || new TextEncoder().encode(rule).length > 200) {
      setError("请输入不含空白字符、200 字节以内的模型名称或规则");
      return;
    }
    if (rules.length >= 200) { setError("发布规则最多 200 条"); return; }
    setRules((previous) => previous.some((item) => item.toLowerCase() === rule.toLowerCase()) ? previous : [...previous, rule]);
    setDraft("");
    setError("");
  }, [rules.length]);

  const save = useCallback(async () => {
    if (!channel) return;
    if (draft.trim() && !publishAll) { setError("请先添加或清空输入中的规则，再保存"); return; }
    if (!publishAll && rules.length === 0) { setError("请至少添加一条发布规则，或开启「发布全部支持模型」"); return; }
    setPending(true);
    setError("");
    try {
      await updateChannel(channel.id, { modelAllow: publishAll ? [] : rules });
      toast.success("对外模型配置已保存");
      onSaved();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally { setPending(false); }
  }, [channel, draft, onSaved, publishAll, rules]);

  return <Dialog open={channel !== null} onOpenChange={(open) => { if (!open && !pending) onClose(); }}>
    <DialogContent className="max-w-xl" showCloseButton={!pending} onInteractOutside={(event) => { if (pending) event.preventDefault(); }} onEscapeKeyDown={(event) => { if (pending) event.preventDefault(); }}>
      <DialogHeader>
        <DialogTitle>对外模型 · {channel?.name}</DialogTitle>
        <DialogDescription>账号决定支持哪些模型，发布规则决定本分组开放哪些模型。新模型匹配规则后随账号目录更新。</DialogDescription>
      </DialogHeader>
      <div className="flex min-w-0 flex-col gap-3">
        <label className="flex items-center justify-between gap-3 text-sm">
          发布全部支持模型
          <Switch checked={publishAll} onCheckedChange={(checked) => { setPublishAll(checked); setError(""); }} disabled={pending} aria-label="发布全部支持模型" />
        </label>
        {publishAll ? <p className="text-xs text-muted-foreground">发布本分组账号支持的全部模型，包含其他厂商。关闭后可限定模型范围。</p> : <>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-xs font-medium">发布规则 · {rules.length} 条</span>
            {recommendations.length > 0 ? <Button size="sm" variant="outline" disabled={pending} onClick={() => { setRules(recommendations); setError(""); }}>使用厂商推荐规则</Button> : null}
          </div>
          <p className="text-xs leading-5 text-muted-foreground">支持精确名称和 * 通配符，忽略大小写。例如 deepseek*、qwen*；可提前添加新模型。厂商推荐规则可自行调整。</p>
          <div className="flex gap-2">
            <input aria-label="模型名称或发布规则" value={draft} disabled={pending} onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addRule(draft); } }}
              placeholder="例如 deepseek* 或 qwen4" className="h-8 min-w-0 flex-1 rounded-md border border-input bg-background px-2.5 font-mono text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" />
            <Button size="sm" variant="outline" disabled={pending} onClick={() => addRule(draft)}>添加</Button>
          </div>
          <div className="flex max-h-36 flex-wrap gap-1.5 overflow-auto rounded-md border p-2" aria-label="已配置发布规则">
            {rules.length === 0 ? <p className="text-xs text-muted-foreground">尚未添加发布规则</p> : rules.map((rule) => <span key={rule} className="inline-flex max-w-full items-center gap-1 rounded bg-muted py-1 pr-1 pl-2 text-xs">
              <span className="min-w-0 break-all font-mono">{rule}</span>
              <button type="button" disabled={pending} aria-label={`移除规则 ${rule}`} onClick={() => { setRules((previous) => previous.filter((item) => item !== rule)); setError(""); }}
                className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><X className="size-3.5" aria-hidden="true" /></button>
            </span>)}
          </div>
        </>}
        <div className="flex items-center justify-between gap-2 border-t pt-3">
          <span className="text-xs font-medium">账号模型 · {preview.filter((model) => model.matched).length} / {models.length} 个匹配</span>
          <Button size="sm" variant="ghost" disabled={pending || loading} onClick={() => setReload((value) => value + 1)}><RefreshCw className="size-3.5" aria-hidden="true" />刷新目录</Button>
        </div>
        <MyBox isLoading={loading} text="正在加载分组支持的模型..." className="min-h-24">
          {loadError ? <ErrorState message={loadError} onRetry={() => setReload((value) => value + 1)} /> : models.length === 0 ?
            <p className="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">暂无账号模型。可先保存发布规则，再添加账号或在账号编辑中获取模型。</p> :
            <div className="flex max-h-40 flex-wrap gap-2 overflow-auto rounded-md border p-3">
              {preview.map(({ name, matched }) => <button key={name} type="button" aria-pressed={matched} disabled={pending || publishAll || matched}
                title={matched ? "已匹配发布规则" : "添加为精确发布规则"} onClick={() => addRule(name)}
                className={cn("max-w-full rounded-md border px-2 py-1 text-left font-mono text-xs break-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", matched ? "border-primary/40 bg-primary/10 text-primary" : "text-muted-foreground hover:bg-muted")}>{name}</button>)}
            </div>}
        </MyBox>
        {!publishAll && !loading && !loadError && waiting.length > 0 ? <p className="text-xs leading-5 text-muted-foreground">{waiting.length} 条规则暂未匹配账号模型，已保留。账号后续支持时才会发布具体模型；通配规则不会自动加入探活。</p> : null}
      </div>
      {error ? <ErrorState title="保存失败" message={error} /> : null}
      <DialogFooter>
        <Button size="sm" variant="outline" onClick={onClose} disabled={pending}>取消</Button>
        <Button size="sm" onClick={() => void save()} disabled={pending}>{pending ? "保存中..." : "保存"}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
};
