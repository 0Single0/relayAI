import { useEffect, useMemo, useState } from "react";
import { createChannel, updateChannel, type ChannelPayload } from "@/api/channels";
import type { Channel } from "@/api/types";
import { ErrorState } from "@/components/common/ErrorState";
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
import { Switch } from "@/components/ui/switch";
import { ChannelStatusSettings, type StatusSettingsValue } from "@/pages/Channels/components/ChannelStatusSettings";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

type ChannelFormSheetProps = {
  open: boolean;
  channel: Channel | null;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
};

type FormState = {
  id: string;
  name: string;
  enabled: boolean;
  priority: string;
  weight: string;
  channelRatio: string;
  concurrencyLimit: string;
  rateLimitQps: string;
  /**
   * 这个分组用哪个适配器。
   *
   * **不是"随便填一个满足接口"的字段**：它决定用哪套上游协议，
   * 也决定运营能力（任务 / 签到只有 workbuddy 国内版有）。
   * 早先固定传 `custom`，结果 WorkBuddy 分组的任务菜单整个消失 ——
   * 因为 `custom` 适配器不声明任何任务能力。
   */
  adapter: Channel["adapter"];
  /** WorkBuddy 专用：国内版 / 国际版。两者的上游域名与任务完全不同。 */
  realm: "cn" | "intl";
  /** 「分组状态」的展示开关与探活模型。 */
  status: StatusSettingsValue;
};

/**
 * 新建分组的初始状态。
 *
 * `status.visible` 默认 **false**：分组是调度用的计价桶，
 * 出现在「分组状态」页是额外动作，不该因为建了分组就自动公开。
 */
const emptyForm: FormState = {
  id: "",
  name: "",
  enabled: true,
  priority: "100",
  weight: "1",
  channelRatio: "1.00",
  concurrencyLimit: "",
  rateLimitQps: "",
  adapter: "workbuddy",
  realm: "cn",
  status: { visible: false, probeModels: [] },
};

const fromChannel = (channel: Channel): FormState => ({
  id: channel.id,
  name: channel.name,
  enabled: channel.enabled,
  priority: String(channel.priority),
  weight: String(channel.weight),
  channelRatio: channel.channelRatio.toFixed(2),
  concurrencyLimit: channel.concurrencyLimit === null ? "" : String(channel.concurrencyLimit),
  rateLimitQps: channel.rateLimitQps === null ? "" : String(channel.rateLimitQps),
  adapter: channel.adapter,
  realm: (channel.options?.realm === "intl" ? "intl" : "cn") as "cn" | "intl",
  status: {
    visible: channel.statusVisible === true,
    probeModels: channel.probeModels ?? (channel.probeModel ? [channel.probeModel] : []),
  },
});

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
  "aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20",
);

const Field = ({
  label,
  hint,
  error,
  children,
}: {
  label: string;
  hint?: string;
  error?: string;
  children: React.ReactNode;
}) => (
  <div className="flex flex-col gap-1">
    <label className="text-xs font-medium text-foreground">{label}</label>
    {children}
    {error ? (
      <p className="text-[11px] text-destructive">{error}</p>
    ) : hint ? (
      <p className="text-[11px] leading-4 text-muted-foreground">{hint}</p>
    ) : null}
  </div>
);

/**
 * 新建 / 编辑渠道。
 *
 * 分组固定平台与版本，配置计价倍率和限流；账号单独保存上游凭据。
 * 对外模型在独立弹窗配置，编辑基本信息不会改动发布策略。
 */
export const ChannelFormSheet = ({
  open,
  channel,
  onOpenChange,
  onSaved,
}: ChannelFormSheetProps) => {
  const [form, setForm] = useState<FormState>(emptyForm);
  const [errors, setErrors] = useState<Partial<Record<keyof FormState, string>>>({});
  const [submitError, setSubmitError] = useState("");
  const [pending, setPending] = useState(false);

  const isEdit = channel !== null;

  useEffect(() => {
    if (!open) {
      return;
    }
    setForm(channel ? fromChannel(channel) : emptyForm);
    setErrors({});
    setSubmitError("");
  }, [channel, open]);

  const update = <K extends keyof FormState>(key: K, value: FormState[K]) => {
    setForm((prev) => ({ ...prev, [key]: value }));
    setErrors((prev) => ({ ...prev, [key]: undefined }));
  };

  const validate = (): boolean => {
    const next: Partial<Record<keyof FormState, string>> = {};

    if (!form.id.trim()) {
      next.id = "渠道 id 不能为空";
    } else if (!/^[a-z0-9][a-z0-9-]*$/.test(form.id.trim())) {
      next.id = "只能用小写字母、数字和短横线";
    }
    if (!form.name.trim()) {
      next.name = "名称不能为空";
    }


    const ratio = Number(form.channelRatio);
    if (Number.isNaN(ratio) || ratio <= 0) {
      next.channelRatio = "倍率需大于 0";
    }

    setErrors(next);
    return Object.keys(next).length === 0;
  };

  const buildPayload = (): ChannelPayload => ({
    id: form.id.trim(),
    name: form.name.trim(),
    // 分组的平台及版本用于限制可归入的账号。
    adapter: form.adapter,
    enabled: form.enabled,
    // 分组级 priority / weight 在界面上已移除（Key 与分组 1:1，永不参与比较），
    // 但接口仍要求这两个字段，固定给中性值。
    priority: 100,
    weight: 1,
    options: form.adapter === "workbuddy" ? { realm: form.realm } : {},
    channelRatio: Number(form.channelRatio),
    concurrencyLimit: form.concurrencyLimit.trim() ? Number(form.concurrencyLimit) : null,
    rateLimitQps: form.rateLimitQps.trim() ? Number(form.rateLimitQps) : null,
    // 展示开关与探活模型一起写：它们是同一块设置的两个面，
    // 分开保存会让"开了展示但一轮都没探过"这种半配置状态留在库里。
    statusVisible: form.status.visible,
    probeModels: form.status.probeModels,
    // 旧版单值字段跟着列表收敛，避免两处口径不一致。
    probeModel: form.status.probeModels[0] ?? "",
  });

  const submit = async () => {
    setSubmitError("");
    if (!validate()) {
      return;
    }

    setPending(true);
    try {
      const payload = buildPayload();
      if (isEdit) {
        const { id: _id, adapter: _adapter, ...rest } = payload;
        await updateChannel(channel.id, rest);
      } else {
        await createChannel(payload);
      }
      onSaved();
    } catch (error) {
      setSubmitError(getRequestErrorMessage(error));
    } finally {
      setPending(false);
    }
  };

  const title = useMemo(
    () => (isEdit ? `编辑渠道 · ${channel?.name ?? ""}` : "新建分组"),
    [channel?.name, isEdit],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            分组用于配置倍率与限流，仅接收相同平台及版本的账号。API Key 在「新建账号」中填写。
          </DialogDescription>
        </DialogHeader>

        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <Field
            label="渠道 id"
            error={errors.id}
            hint={isEdit ? "创建后不可修改" : "小写字母、数字、短横线"}
          >
            <input
              value={form.id}
              onChange={(event) => update("id", event.target.value)}
              disabled={isEdit}
              aria-invalid={errors.id ? true : undefined}
              placeholder="cheap-lane"
              className={cn(inputClass, "font-mono", isEdit && "opacity-60")}
            />
          </Field>

          <Field label="显示名称" error={errors.name}>
            <input
              value={form.name}
              onChange={(event) => update("name", event.target.value)}
              aria-invalid={errors.name ? true : undefined}
              placeholder="低价通道"
              className={inputClass}
            />
          </Field>

          {/*
            平台：决定用哪套上游协议，也决定运营能力。
            只有 WorkBuddy 有「任务 / 签到」，而且只限国内版 ——
            选错这里，任务菜单会整个消失（不报错，只是没有）。
          */}
          <Field
            label="平台"
            hint={
              form.adapter === "workbuddy"
                ? form.realm === "cn"
                  ? "国内版：支持任务与签到"
                  : "国际版：无任务，只有每日对话"
                : form.adapter === "trae" ? "Trae：支持签到" : "自定义供应商：无运营能力"
            }
          >
            <Select disabled={isEdit} value={form.adapter} onValueChange={(next) => update("adapter", next as Channel["adapter"])}>
              <SelectTrigger aria-label="平台">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="workbuddy">WorkBuddy</SelectItem>
                <SelectItem value="trae">Trae</SelectItem>
                <SelectItem value="custom">自定义供应商</SelectItem>
              </SelectContent>
            </Select>
          </Field>

          {form.adapter === "workbuddy" ? (
            <Field
              label="版本"
              hint="国内版与国际版的上游域名不同，账号不能混用"
            >
              <Select
                disabled={isEdit}
                value={form.realm}
                onValueChange={(next) => update("realm", next as "cn" | "intl")}
              >
                <SelectTrigger aria-label="版本">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="cn">国内版</SelectItem>
                  <SelectItem value="intl">国际版</SelectItem>
                </SelectContent>
              </Select>
            </Field>
          ) : null}

          <Field label="启用">
            <div className="flex h-8 items-center gap-2">
              <Switch
                checked={form.enabled}
                onCheckedChange={(checked) => update("enabled", checked)}
                aria-label="启用渠道"
              />
              <span className="text-xs text-muted-foreground">
                {form.enabled ? "已启用" : "已停用"}
              </span>
            </div>
          </Field>

          <Field
            label="渠道倍率"
            error={errors.channelRatio}
            hint="记账 = 上游成本 × 模型倍率 × 本倍率"
          >
            <input
              value={form.channelRatio}
              onChange={(event) => update("channelRatio", event.target.value)}
              inputMode="decimal"
              aria-invalid={errors.channelRatio ? true : undefined}
              className={cn(inputClass, "tabular-nums")}
            />
          </Field>

          {/*
            这里**没有**优先级 / 权重。

            为什么删掉：它们本来用于"多个分组提供同一模型时选哪个"，
            但本系统里 Key 与分组是 1:1 强制绑定的 —— 后端要求
            `ChannelAllow` 长度恰好为 1（见 handler/keys.go），
            所以候选集在进入路由前就被 Key 白名单收窄成**只剩一个分组**，
            分组的 priority/weight 永远不可能参与比较。

            把一个永远不生效的开关摆成必填项，只会让人以为"调它能改调度"。
            真正生效的是**账号级**的优先级与权重（同一分组内多个号之间的分工），
            那两个字段在「账号管理」里。
          */}

          <Field label="并发上限" hint="留空表示不限制">
            <input
              value={form.concurrencyLimit}
              onChange={(event) => update("concurrencyLimit", event.target.value)}
              inputMode="numeric"
              placeholder="不限"
              className={cn(inputClass, "tabular-nums")}
            />
          </Field>

          <Field label="QPS 上限" hint="留空表示不限制">
            <input
              value={form.rateLimitQps}
              onChange={(event) => update("rateLimitQps", event.target.value)}
              inputMode="numeric"
              placeholder="不限"
              className={cn(inputClass, "tabular-nums")}
            />
          </Field>
        </div>

        {/*
          「分组状态」的展示开关与探活模型。

          放在表单里而不是留在状态页：那是"分组的一个属性"（要不要公开、
          用哪个模型证明可用），和倍率、启停属于同一层。状态页本身只负责
          呈现，藏起来的分组在那里连入口都没有，设置就永远改不回来。
        */}
        <ChannelStatusSettings
          // 新建时还不存在这个分组，也就没有"它对外提供的模型"。
          // 传空串让这块只显示说明，不去拉一份必然为空的目录。
          channelId={isEdit ? channel.id : ""}
          value={form.status}
          disabled={pending}
          onChange={(next) => update("status", next)}
        />

        {submitError ? <ErrorState title="保存失败" message={submitError} /> : null}

        <DialogFooter>
          <Button
            variant="outline"
            size="sm"
            onClick={() => onOpenChange(false)}
            disabled={pending}
          >
            取消
          </Button>
          <Button size="sm" onClick={() => void submit()} disabled={pending}>
            {pending ? "保存中..." : "保存"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
