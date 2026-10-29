import { useCallback, useEffect, useState } from "react";
import { updateAccount } from "@/api/accounts";
import { setAccountProxy, type Proxy } from "@/api/proxies";
import type { Account, Channel } from "@/api/types";
import { ErrorState } from "@/components/common/ErrorState";
import { toast } from "@/components/common/Toast/store";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { accountMatchesChannel, accountPlatformLabel, outboundOptions, upstreamProtocolOptions } from "@/pages/Channels/shared";
import { getRequestErrorMessage } from "@/utils/request";
import { CustomModelPicker } from "./CustomModelPicker";
import { createModelDraft, getCustomModelError, serializeCustomModels, type CustomModelDraft } from "./customModels";

const inputClass = "h-8 w-full min-w-0 rounded-md border border-input bg-background px-2.5 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/30";
type Props = { account: Account | null; channels: Channel[]; proxies: Proxy[]; onClose: () => void; onDone: () => void };

export const EditAccountDialog = ({ account, channels, proxies, onClose, onDone }: Props) => {
  const [nickname, setNickname] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [channelId, setChannelId] = useState("");
  const [maxInflight, setMaxInflight] = useState("");
  const [priority, setPriority] = useState("");
  const [weight, setWeight] = useState("");
  const [proxyId, setProxyId] = useState<number | null>(null);
  const [baseUrl, setBaseUrl] = useState("");
  const [protocol, setProtocol] = useState("openai-chat");
  const [apiKey, setApiKey] = useState("");
  const [outbound, setOutbound] = useState("desktop");
  const [models, setModels] = useState<CustomModelDraft[]>([]);
  const [modelsPending, setModelsPending] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const platform = account?.platform || account?.adapter;
  const custom = platform === "custom";

  useEffect(() => {
    setApiKey("");
    if (!account) return;
    setNickname(account.nickname);
    setEnabled(account.enabled);
    setChannelId(account.channelId);
    setMaxInflight(account.maxInflight == null ? "" : String(account.maxInflight));
    setPriority(account.priorityOverride == null ? "" : String(account.priorityOverride));
    setWeight(account.weightOverride == null ? "" : String(account.weightOverride));
    setProxyId(account.proxyId);
    setBaseUrl(account.baseUrl || account.options?.base_url || "");
    setProtocol(account.protocol || account.options?.protocol || "openai-chat");
    setOutbound(account.options?.outbound_identity || "desktop");
    const entries = new Map((account.options?.models ?? []).map(entry => {
      const model = typeof entry === "string" ? { id: entry } : entry;
      return [model.id, model] as const;
    }));
    const ids = account.modelAllow.length ? account.modelAllow : [...entries.keys()];
    setModels(ids.map(id => createModelDraft(entries.get(id) ?? { id })));
    setError("");
  }, [account]);

  const submit = useCallback(async () => {
    if (!account || pending) return;
    const numeric = [["并发上限", maxInflight, 1], ["调度优先级", priority, 0], ["调度权重", weight, 1]] as const;
    for (const [label, value, minimum] of numeric) {
      if (value.trim() && (!/^\d+$/.test(value.trim()) || Number(value) < minimum || Number(value) > 2147483647)) {
        setError(`${label}必须为${minimum ? "正" : "非负"}整数，留空恢复默认`); return;
      }
    }
    if (custom) {
      try {
        const url = new URL(baseUrl.trim());
        if (!/^https?:$/.test(url.protocol) || url.username || url.password) throw new Error();
      } catch { setError("请填写有效的 HTTP 或 HTTPS 上游地址"); return; }
      const modelError = getCustomModelError(models);
      if (modelError) { setError(modelError); return; }
    }
    setPending(true); setError("");
    try {
      await updateAccount(account.id, {
        nickname: nickname.trim(), enabled,
        maxInflight: maxInflight.trim() ? Number(maxInflight) : null,
        priorityOverride: priority.trim() ? Number(priority) : null,
        weightOverride: weight.trim() ? Number(weight) : null,
        ...(channelId !== account.channelId ? { channelId } : {}),
        ...(custom ? {
          baseUrl: baseUrl.trim(), protocol,
          ...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
          modelAllow: models.map(model => model.id.trim()),
          options: { ...account.options, models: serializeCustomModels(models) },
        } : {
          ...(platform?.startsWith("workbuddy") ? { options: { ...account.options, outbound_identity: outbound as "desktop" | "cli" | "vscode" } } : {}),
        }),
      });
      if (proxyId !== account.proxyId) await setAccountProxy(account.id, proxyId);
      toast.success("账号已更新");
      onDone();
    } catch (requestError) { setError(getRequestErrorMessage(requestError)); }
    finally { setPending(false); }
  }, [account, pending, maxInflight, priority, weight, custom, baseUrl, models, nickname, enabled, channelId, protocol, apiKey, platform, outbound, proxyId, onDone]);

  return <Dialog open={account !== null} onOpenChange={next => { if (!next && !pending) onClose(); }}>
    <DialogContent className="max-w-2xl" showCloseButton={!pending}>
      <DialogHeader><DialogTitle>编辑账号</DialogTitle><DialogDescription>{account ? `${accountPlatformLabel(account)} · ${account.uid}` : ""}</DialogDescription></DialogHeader>
      <fieldset disabled={pending} className="flex min-w-0 flex-col gap-4">
        <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2">
          <label htmlFor="edit-account-nickname" className="flex min-w-0 flex-col gap-1 text-xs font-medium">昵称<input id="edit-account-nickname" value={nickname} onChange={event => setNickname(event.target.value)} className={inputClass} /></label>
          <div className="flex min-w-0 flex-col gap-1"><label id="edit-account-status" className="text-xs font-medium">状态</label><Select disabled={pending} value={enabled ? "on" : "off"} onValueChange={next => setEnabled(next === "on")}><SelectTrigger aria-labelledby="edit-account-status"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="on">启用</SelectItem><SelectItem value="off">停用（不参与调度）</SelectItem></SelectContent></Select></div>
          <div className="flex min-w-0 flex-col gap-1 sm:col-span-2"><label id="edit-account-group" className="text-xs font-medium">所属分组</label><Select disabled={pending} value={channelId || "__none__"} onValueChange={value => setChannelId(value === "__none__" ? "" : value)}><SelectTrigger aria-label="所属分组"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="__none__">暂不分配</SelectItem>{channels.filter(channel => account && accountMatchesChannel(account, channel) && (channel.enabled || channel.id === account.channelId)).map(channel => <SelectItem key={channel.id} value={channel.id} disabled={!channel.enabled}>{channel.name}（×{channel.channelRatio.toFixed(2)}）</SelectItem>)}</SelectContent></Select><p className="text-[11px] text-muted-foreground">仅可选择相同平台及版本的分组；未分配账号不参与对外调度。</p></div>
          <label htmlFor="edit-account-concurrency" className="flex min-w-0 flex-col gap-1 text-xs font-medium">并发上限<input id="edit-account-concurrency" value={maxInflight} onChange={event => setMaxInflight(event.target.value)} inputMode="numeric" placeholder="留空使用系统默认" className={inputClass} /><span className="text-[11px] font-normal text-muted-foreground">同时处理的请求数上限；列表显示当前请求数 / 生效上限。</span></label>
          <div className="flex min-w-0 flex-col gap-1"><label id="edit-account-proxy" className="text-xs font-medium">出站代理</label><Select disabled={pending} value={proxyId == null ? "__direct__" : String(proxyId)} onValueChange={value => setProxyId(value === "__direct__" ? null : Number(value))}><SelectTrigger aria-labelledby="edit-account-proxy"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="__direct__">直连（用服务器出口 IP）</SelectItem>{proxies.map(proxy => <SelectItem key={proxy.id} value={String(proxy.id)}>{proxy.name} · {proxy.protocol}://{proxy.host}:{proxy.port}</SelectItem>)}</SelectContent></Select></div>
          <label htmlFor="edit-account-priority" className="flex min-w-0 flex-col gap-1 text-xs font-medium">调度优先级<input id="edit-account-priority" value={priority} onChange={event => setPriority(event.target.value)} inputMode="numeric" className={inputClass} /></label>
          <label htmlFor="edit-account-weight" className="flex min-w-0 flex-col gap-1 text-xs font-medium">调度权重<input id="edit-account-weight" value={weight} onChange={event => setWeight(event.target.value)} inputMode="numeric" className={inputClass} /></label>
        </div>
        {platform?.startsWith("workbuddy") ? <div className="flex flex-col gap-1"><label id="edit-account-outbound" className="text-xs font-medium">出站身份</label><Select disabled={pending} value={outbound} onValueChange={setOutbound}><SelectTrigger aria-labelledby="edit-account-outbound"><SelectValue /></SelectTrigger><SelectContent>{outboundOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent></Select></div> : null}
        {custom ? <>
          <label htmlFor="edit-account-url" className="flex min-w-0 flex-col gap-1 text-xs font-medium">上游地址<input id="edit-account-url" value={baseUrl} onChange={event => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" className={inputClass} /></label>
          <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2"><div className="flex min-w-0 flex-col gap-1"><label id="edit-account-protocol" className="text-xs font-medium">上游协议</label><Select disabled={pending} value={protocol} onValueChange={setProtocol}><SelectTrigger aria-labelledby="edit-account-protocol"><SelectValue /></SelectTrigger><SelectContent>{upstreamProtocolOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent></Select></div>
          <label htmlFor="edit-account-key" className="flex min-w-0 flex-col gap-1 text-xs font-medium">替换 API Key<input id="edit-account-key" type="password" autoComplete="new-password" value={apiKey} onChange={event => setApiKey(event.target.value)} placeholder="留空保留现有密钥" className={inputClass} /><span className="text-[11px] font-normal text-muted-foreground">现有密钥不会回显；获取模型时可使用已保存的密钥。</span></label></div>
          <CustomModelPicker baseUrl={baseUrl} apiKey={apiKey} protocol={protocol} accountId={account?.id} value={models} onChange={setModels} disabled={pending} onFetchingChange={setModelsPending} />
        </> : null}
      </fieldset>
      {error ? <ErrorState title="保存失败" message={error} /> : null}
      <DialogFooter><Button variant="outline" size="sm" onClick={onClose} disabled={pending}>取消</Button><Button size="sm" onClick={() => void submit()} disabled={pending || modelsPending}>{pending ? "保存中..." : "保存"}</Button></DialogFooter>
    </DialogContent>
  </Dialog>;
};
