import { useEffect, useMemo, useState } from "react";
import { createKey, updateKey, type KeyPayload } from "@/api/keys";
import type { CatalogModel } from "@/api/catalog";
import type { ApiKey, Channel, Model } from "@/api/types";
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
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

type KeyFormDialogProps = {
  open: boolean;
  apiKey: ApiKey | null;
  channels: Channel[];
  models: Model[];
  /** 模型 × 分组 归属（来自 `/catalog/models`），用于按分组收窄可选项。 */
  catalogModels: CatalogModel[];
  onOpenChange: (open: boolean) => void;
  onSaved: (plaintext?: string) => void;
};

type FormState = {
  name: string;
  channelAllow: string[];
  modelAllow: string[];
  rpmLimit: string;
  dailyTokenLimit: string;
  maxConcurrency: string;
  enabled: boolean;
};

const emptyForm: FormState = {
  name: "",
  channelAllow: [],
  modelAllow: [],
  rpmLimit: "",
  dailyTokenLimit: "",
  maxConcurrency: "",
  enabled: true,
};

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

export const KeyFormDialog = ({
  open,
  apiKey,
  channels,
  models,
  catalogModels,
  onOpenChange,
  onSaved,
}: KeyFormDialogProps) => {
  const [form, setForm] = useState<FormState>(emptyForm);
  const [error, setError] = useState("");
  const [nameError, setNameError] = useState("");
  const [pending, setPending] = useState(false);

  const isEdit = apiKey !== null;

  useEffect(() => {
    if (!open) {
      return;
    }

    setError("");
    setNameError("");
    setForm(
      apiKey
        ? {
            name: apiKey.name,
            channelAllow: apiKey.channelAllow,
            modelAllow: apiKey.modelAllow,
            rpmLimit: apiKey.rpmLimit === null ? "" : String(apiKey.rpmLimit),
            dailyTokenLimit:
              apiKey.dailyTokenLimit === null ? "" : String(apiKey.dailyTokenLimit),
            maxConcurrency:
              apiKey.maxConcurrency === null ? "" : String(apiKey.maxConcurrency),
            enabled: apiKey.enabled,
          }
        : emptyForm,
    );
  }, [apiKey, open]);

  /**
   * 模型选项随渠道联动（SPEC 9.12）：
   * 选了渠道后，只能勾选这些渠道实际提供的模型。
   */
  /**
   * 可选模型 = 所选分组**对外提供**的模型。
   *
   * 之前这里两个分支都 `return models`（全量），等于没过滤 ——
   * 于是 Key 的模型白名单里能看到这个分组根本不提供的模型，
   * 选完调过去必然 403。这正是"感觉好像是全部的了"的来源。
   *
   * 规则：
   *   · 没选分组 → 显示全部（Key 必须绑分组，这只是过渡态）；
   *   · 选了分组 → 只显示该分组 `exposed` 的模型；
   *   · 分组一个模型都没有（账号全停用）→ 也显示空，让人看出问题。
   */
  const availableModels = useMemo(() => {
    if (form.channelAllow.length === 0) {
      return models;
    }

    const allowed = new Set<string>();
    for (const model of models) {
      // `models` 来自 `/models`（全量，不含渠道归属），
      // 归属信息在 `catalogModels` 里 —— 这里用后者判断。
      const catalog = catalogModels.find((item) => item.name === model.canonicalName);
      if (!catalog) {
        continue;
      }
      const exposed = catalog.channels.some(
        (channel) => channel.exposed && form.channelAllow.includes(channel.id),
      );
      if (exposed) {
        allowed.add(model.canonicalName);
      }
    }

    return models.filter((model) => allowed.has(model.canonicalName));
  }, [catalogModels, form.channelAllow, models]);

  const toggleModel = (name: string) => {
    setForm((prev) => ({
      ...prev,
      modelAllow: prev.modelAllow.includes(name)
        ? prev.modelAllow.filter((entry) => entry !== name)
        : [...prev.modelAllow, name],
    }));
  };

  const submit = async () => {
    setError("");
    setNameError("");

    if (!form.name.trim()) {
      setNameError("名称不能为空");
      return;
    }

    const payload: KeyPayload = {
      name: form.name.trim(),
      channelAllow: form.channelAllow,
      modelAllow: form.modelAllow,
      rpmLimit: form.rpmLimit.trim() ? Number(form.rpmLimit) : null,
      dailyTokenLimit: form.dailyTokenLimit.trim() ? Number(form.dailyTokenLimit) : null,
      maxConcurrency: form.maxConcurrency.trim() ? Number(form.maxConcurrency) : null,
      expiresAt: apiKey?.expiresAt ?? null,
    };

    setPending(true);
    try {
      if (isEdit) {
        await updateKey(apiKey.id, { ...payload, enabled: form.enabled });
        onSaved();
      } else {
        const response = await createKey(payload);
        onSaved(response.data?.plaintext);
      }
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>{isEdit ? `编辑密钥 · ${apiKey?.name}` : "新建密钥"}</DialogTitle>
          <DialogDescription>
            一把密钥绑一个渠道。不绑定则由调度在全部渠道里按权重选。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">名称</label>
            <input
              value={form.name}
              onChange={(event) => {
                setForm((prev) => ({ ...prev, name: event.target.value }));
                setNameError("");
              }}
              aria-invalid={nameError ? true : undefined}
              placeholder="Cursor 专用"
              className={inputClass}
            />
            {nameError ? (
              <p className="text-[11px] text-destructive">{nameError}</p>
            ) : null}
          </div>

          {/*
            渠道**必须选一个**，没有"不绑定/全部渠道"这个选项。
            一把密钥对应一个渠道，这是"用便宜的 Key / 稳定的 Key"分流的前提；
            允许留空等于允许"这把 Key 能走任意通道"，那分流就不成立了。
          */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium">渠道</label>
            <Select
              value={form.channelAllow[0] ?? ""}
              onValueChange={(value) =>
                setForm((prev) => ({ ...prev, channelAllow: [value] }))
              }
            >
              <SelectTrigger aria-label="绑定渠道">
                <SelectValue placeholder="选择渠道" />
              </SelectTrigger>
              <SelectContent>
                {channels.map((channel) => (
                  <SelectItem key={channel.id} value={channel.id}>
                    {channel.name}（×{channel.channelRatio.toFixed(2)}）
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {channels.length === 0 ? (
              <p className="text-[11px] text-destructive">
                还没有渠道。先去「渠道管理」建一个，再回来创建密钥。
              </p>
            ) : null}
          </div>

          <fieldset className="flex flex-col gap-1.5">
            <legend className="text-xs font-medium">
              模型白名单
              <span className="ml-1 font-normal text-muted-foreground">
                （不选 = 全部模型）
              </span>
            </legend>
            <div className="max-h-32 overflow-y-auto rounded-md border p-2">
              {availableModels.length === 0 ? (
                <p className="px-1 text-xs text-muted-foreground">
                  {form.channelAllow.length === 0
                    ? "模型目录尚未同步。"
                    : "该分组当前没有对外提供的模型：先给分组添加账号，网关拉到模型目录后再回来配。"}
                </p>
              ) : (
                <div className="flex flex-wrap gap-1.5">
                  {availableModels.map((model) => {
                    const checked = form.modelAllow.includes(model.canonicalName);
                    return (
                      <button
                        key={model.canonicalName}
                        type="button"
                        onClick={() => toggleModel(model.canonicalName)}
                        aria-pressed={checked}
                        className={cn(
                          "rounded-full border px-2 py-0.5 font-mono text-[11px] transition-colors",
                          "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                          checked
                            ? "border-primary bg-primary/10 text-primary"
                            : "border-border text-muted-foreground hover:border-border-strong",
                        )}
                      >
                        {model.canonicalName}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </fieldset>

          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">RPM 限制</label>
              <input
                value={form.rpmLimit}
                onChange={(event) =>
                  setForm((prev) => ({ ...prev, rpmLimit: event.target.value }))
                }
                inputMode="numeric"
                placeholder="不限"
                className={cn(inputClass, "tabular-nums")}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">每日 Token</label>
              <input
                value={form.dailyTokenLimit}
                onChange={(event) =>
                  setForm((prev) => ({ ...prev, dailyTokenLimit: event.target.value }))
                }
                inputMode="numeric"
                placeholder="不限"
                className={cn(inputClass, "tabular-nums")}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">最大并发</label>
              <input
                value={form.maxConcurrency}
                onChange={(event) =>
                  setForm((prev) => ({ ...prev, maxConcurrency: event.target.value }))
                }
                inputMode="numeric"
                placeholder="不限"
                className={cn(inputClass, "tabular-nums")}
              />
            </div>
          </div>

          {isEdit ? (
            <div className="flex items-center gap-2">
              <Switch
                checked={form.enabled}
                onCheckedChange={(checked) =>
                  setForm((prev) => ({ ...prev, enabled: checked }))
                }
                aria-label="启用密钥"
              />
              <span className="text-xs text-muted-foreground">
                {form.enabled ? "已启用" : "已停用"}
              </span>
            </div>
          ) : null}
        </div>

        {error ? <ErrorState title="保存失败" message={error} /> : null}

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
