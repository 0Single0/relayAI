import { useCallback, useEffect, useRef, useState } from "react";
import { Download, Plus } from "lucide-react";
import { discoverModels } from "@/api/accounts";
import type { CustomModelDefinition } from "@/api/accounts/models";
import { Button } from "@/components/ui/button";
import { getRequestErrorMessage } from "@/utils/request";
import { CustomModelRow } from "./CustomModelRow";
import { ModelSelectionDialog } from "./ModelSelectionDialog";
import { createModelDraft, type CustomModelDraft } from "./customModels";

type Props = {
  baseUrl: string; apiKey: string; protocol: string;
  accountId?: number;
  value: CustomModelDraft[]; onChange: (models: CustomModelDraft[]) => void;
  disabled: boolean; onFetchingChange: (fetching: boolean) => void;
};

export const CustomModelPicker = ({ baseUrl, apiKey, protocol, accountId, value, onChange, disabled, onFetchingChange }: Props) => {
  const [fetching, setFetching] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [selectionOpen, setSelectionOpen] = useState(false);
  const [available, setAvailable] = useState<CustomModelDefinition[]>([]);
  const request = useRef(0);

  useEffect(() => {
    request.current += 1;
    setError(""); setMessage(""); setFetching(false); setSelectionOpen(false); setAvailable([]); onFetchingChange(false);
    return () => { request.current += 1; onFetchingChange(false); };
  }, [baseUrl, apiKey, protocol, onFetchingChange]);

  const fetchModels = useCallback(async () => {
    const current = ++request.current;
    setSelectionOpen(true); setFetching(true); onFetchingChange(true); setError(""); setMessage(""); setAvailable([]);
    try {
      const response = await discoverModels({ baseUrl: baseUrl.trim(), apiKey: apiKey.trim(), protocol, accountId });
      if (request.current !== current) return;
      const seen = new Set<string>();
      const discovered: CustomModelDefinition[] = [];
      for (const model of response.data ?? []) {
        const id = model.id.trim();
        if (id && !seen.has(id)) { discovered.push({ ...model, id }); seen.add(id); }
      }
      setAvailable(discovered);
    } catch (requestError) {
      if (request.current === current) setError(getRequestErrorMessage(requestError));
    } finally {
      if (request.current === current) { setFetching(false); onFetchingChange(false); }
    }
  }, [accountId, apiKey, baseUrl, onFetchingChange, protocol]);

  return <div className="flex min-w-0 flex-col gap-2">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <span className="text-xs font-medium">支持模型 <span className="text-muted-foreground">（必填）</span></span>
      <ModelSelectionDialog trigger={<Button data-fetch-models type="button" variant="outline" size="sm" onClick={() => void fetchModels()} disabled={disabled || fetching || !baseUrl.trim() || (!apiKey.trim() && !accountId)}><Download aria-hidden="true" />{fetching ? "获取中..." : "获取模型列表"}</Button>}
        open={selectionOpen} onOpenChange={open => {
          setSelectionOpen(open);
          if (!open && fetching) { request.current += 1; setFetching(false); onFetchingChange(false); }
        }} models={available} existing={value.map(model => model.id.trim())} fetching={fetching} error={error} onRetry={() => void fetchModels()} onConfirm={selected => {
          const existing = new Set(value.map(model => model.id.trim()));
          const added = selected.filter(model => !existing.has(model.id)).map(createModelDraft);
          onChange([...value, ...added]); setSelectionOpen(false); setMessage(`已添加 ${added.length} 个模型，已有配置已保留。`);
        }} />
    </div>
    <p className="text-[11px] leading-5 text-muted-foreground">获取后可逐个展开配置，也可手动添加。模型 ID 用于请求上游，显示名称用于模型目录。</p>
      <div className="flex min-h-12 min-w-0 flex-col gap-2">
        {value.length ? value.map((model, index) => <CustomModelRow key={model.key} model={model} index={index} disabled={disabled || fetching}
          onChange={next => onChange(value.map(item => item.key === model.key ? next : item))} onRemove={() => onChange(value.filter(item => item.key !== model.key))} />) : <p className="rounded-md border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">尚未添加模型</p>}
      </div>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <Button type="button" size="sm" variant="outline" disabled={disabled || fetching} onClick={() => onChange([...value, createModelDraft()])}><Plus aria-hidden="true" />添加模型</Button>
      <span className="text-[11px] text-muted-foreground">已添加 {value.length} 个模型</span>
    </div>
    {message ? <p role="status" className="text-[11px] leading-5 text-muted-foreground">{message}</p> : null}
  </div>;
};
