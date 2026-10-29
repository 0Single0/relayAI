import { useId, useState } from "react";
import { ChevronDown, ChevronRight, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { defaultLevels, parseTokenLimit, reasoningLevels, type CustomModelDraft } from "./customModels";

const inputClass = "h-8 w-full min-w-0 rounded-md border border-input bg-background px-2.5 text-xs text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/30 disabled:opacity-50";
type Props = { model: CustomModelDraft; index: number; disabled: boolean; onChange: (model: CustomModelDraft) => void; onRemove: () => void };

export const CustomModelRow = ({ model, index, disabled, onChange, onRemove }: Props) => {
  const [expanded, setExpanded] = useState(!model.id);
  const id = useId();
  const update = (patch: Partial<CustomModelDraft>) => onChange({ ...model, ...patch });
  const invalidContext = Number.isNaN(parseTokenLimit(model.context));
  const invalidOutput = Number.isNaN(parseTokenLimit(model.output));
  return (
    <div className="min-w-0 rounded-lg border bg-muted/15 p-2.5" data-model-row={index}>
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] items-end gap-1.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto_auto]">
        <label className="col-start-1 row-start-1 flex min-w-0 flex-col gap-1 text-[11px] text-muted-foreground" htmlFor={`${id}-model`}>模型 ID
          <input id={`${id}-model`} data-field="model-id" value={model.id} disabled={disabled} autoFocus={!model.id} placeholder="例如：gpt-4.1" className={`${inputClass} font-mono`} onChange={event => update({ id: event.target.value, ...(model.name === model.id ? { name: event.target.value } : {}) })} />
        </label>
        <label className="col-start-1 row-start-2 flex min-w-0 flex-col gap-1 text-[11px] text-muted-foreground sm:col-start-2 sm:row-start-1" htmlFor={`${id}-name`}>显示名称
          <input id={`${id}-name`} data-field="model-name" value={model.name} disabled={disabled} placeholder="默认使用模型 ID" onChange={event => update({ name: event.target.value })} className={inputClass} />
        </label>
        <Button type="button" variant="ghost" size="icon-sm" className="col-start-2 row-start-1 sm:col-start-3" aria-label={`${expanded ? "收起" : "展开"}模型 ${index + 1} 配置`} aria-expanded={expanded} aria-controls={`${id}-settings`} onClick={() => setExpanded(!expanded)}>{expanded ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}</Button>
        <Button type="button" variant="ghost" size="icon-sm" className="col-start-3 row-start-1 sm:col-start-4" disabled={disabled} aria-label={`删除模型 ${index + 1}`} onClick={onRemove}><Trash2 aria-hidden="true" /></Button>
      </div>
      {expanded ? <div id={`${id}-settings`} className="mt-3 flex min-w-0 flex-col gap-3">
        <div className="grid min-w-0 grid-cols-1 gap-2 sm:grid-cols-2">
          <label htmlFor={`${id}-context`} className="flex min-w-0 flex-col gap-1 text-xs">上下文窗口
            <input id={`${id}-context`} data-field="context" aria-invalid={invalidContext} aria-describedby={`${id}-token-help`} disabled={disabled} value={model.context} onChange={event => update({ context: event.target.value })} placeholder="例如：1M（选填）" className={inputClass} />
          </label>
          <label htmlFor={`${id}-output`} className="flex min-w-0 flex-col gap-1 text-xs">最大输出 token 数
            <input id={`${id}-output`} data-field="output" aria-invalid={invalidOutput} aria-describedby={`${id}-token-help`} disabled={disabled} value={model.output} onChange={event => update({ output: event.target.value })} placeholder="例如：32K（选填）" className={inputClass} />
          </label>
        </div>
        <p id={`${id}-token-help`} className={`-mt-1 text-[11px] ${invalidContext || invalidOutput ? "text-destructive" : "text-muted-foreground"}`}>支持正整数、K（千）和 M（百万）；留空表示未知。</p>
        <fieldset className="flex flex-col gap-2"><legend className="mb-2 text-xs">输入类型</legend><div className="flex flex-wrap gap-4 text-xs">
          <label className="flex items-center gap-1.5 text-muted-foreground"><input type="checkbox" checked disabled className="accent-primary" />文本（必选）</label>
          <label className="flex items-center gap-1.5"><input type="checkbox" data-field="vision" checked={model.vision} disabled={disabled} onChange={event => update({ vision: event.target.checked })} className="accent-primary" />图片</label>
        </div></fieldset>
        <div className="flex items-center justify-between gap-2"><label htmlFor={`${id}-reasoning`} className="text-xs">推理等级</label><Switch id={`${id}-reasoning`} checked={model.reasoning} disabled={disabled} onCheckedChange={reasoning => update({ reasoning, levels: model.levels.length ? model.levels : defaultLevels() })} /></div>
        {model.reasoning ? <div className="flex min-w-0 flex-col gap-2">
          <p className="text-[11px] leading-5 text-muted-foreground">将客户端的推理等级映射为上游 API 参数值。off 表示关闭推理，通常对应 none。</p>
          {model.levels.map((item, levelIndex) => <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-end gap-2" key={item.key}>
            <div className="flex min-w-0 flex-col gap-1"><label id={`${id}-${item.key}-label`} className="text-[11px] text-muted-foreground">推理等级</label>
              <Select disabled={disabled} value={item.level} onValueChange={level => update({ levels: model.levels.map(row => row.key === item.key ? { ...row, level } : row) })}><SelectTrigger aria-labelledby={`${id}-${item.key}-label`} className="h-8 w-full min-w-0"><SelectValue /></SelectTrigger><SelectContent>{[...new Set([...reasoningLevels, item.level])].map(level => <SelectItem key={level} value={level} disabled={level !== item.level && model.levels.some(row => row.level === level)}>{level}</SelectItem>)}</SelectContent></Select>
            </div>
            <label htmlFor={`${id}-${item.key}-value`} className="flex min-w-0 flex-col gap-1 text-[11px] text-muted-foreground">API 参数值<input id={`${id}-${item.key}-value`} data-field="effort-value" className={`${inputClass} font-mono`} value={item.value} disabled={disabled} onChange={event => update({ levels: model.levels.map(row => row.key === item.key ? { ...row, value: event.target.value } : row) })} /></label>
            <Button type="button" size="icon-sm" variant="ghost" aria-label={`删除模型 ${index + 1} 的推理等级 ${levelIndex + 1}`} disabled={disabled} onClick={() => update({ levels: model.levels.filter(row => row.key !== item.key) })}><Trash2 aria-hidden="true" /></Button>
          </div>)}
          <Button type="button" size="sm" variant="outline" className="self-start" disabled={disabled || reasoningLevels.every(level => model.levels.some(item => item.level === level))} onClick={() => { const level = reasoningLevels.find(level => !model.levels.some(item => item.level === level)); if (level) update({ levels: [...model.levels, { key: crypto.randomUUID(), level, value: level === "off" ? "none" : level }] }); }}><Plus aria-hidden="true" />添加推理等级</Button>
        </div> : null}
      </div> : null}
    </div>
  );
};
