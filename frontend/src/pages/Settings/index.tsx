import { RefreshCw, Settings as SettingsIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { getSettings, updateSettings } from "@/api/system";
import type { Settings } from "@/api/types";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { toast } from "@/components/common/Toast/store";
import { getRequestErrorMessage } from "@/utils/request";

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm tabular-nums",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

type FieldProps = {
  label: string;
  hint?: string;
  error?: string;
  children: React.ReactNode;
};

const Field = ({ label, hint, error, children }: FieldProps) => (
  <div className="flex flex-col gap-1">
    <label className="text-xs font-medium">{label}</label>
    {children}
    {error ? (
      <p className="text-[11px] text-destructive">{error}</p>
    ) : hint ? (
      <p className="text-[11px] leading-4 text-muted-foreground">{hint}</p>
    ) : null}
  </div>
);

export const SettingsPage = () => {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await getSettings();
      setSettings(response.data ?? null);
      setDraft(response.data ?? null);
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

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setDraft((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  const retentionError = (() => {
    if (!draft) {
      return "";
    }
    const value = draft.taskRetentionDays;
    if (!Number.isInteger(value) || value < 7 || value > 365) {
      return "需为 7–365 之间的整数";
    }
    return "";
  })();

  /**
   * Webhook 校验：留空是合法的（= 不发通知）。
   *
   * 非空时必须是 http(s)。不校验协议的话，用户填 `example.com/hook`
   * 会被网关当成相对路径，`reqwest` 直接报错，而界面上看不出原因。
   */
  const webhookError = (() => {
    if (!draft) {
      return "";
    }
    const value = draft.webhookUrl.trim();
    if (value === "") {
      return "";
    }
    if (!/^https?:\/\/\S+$/i.test(value)) {
      return "需以 http:// 或 https:// 开头";
    }
    return "";
  })();

  const save = async () => {
    if (!draft || retentionError || webhookError) {
      return;
    }

    setPending(true);
    try {
      const response = await updateSettings(draft);
      setSettings(response.data ?? draft);
      setDraft(response.data ?? draft);
      // 成功/失败都用 toast：页面顶部的行内提示会被后续滚动带出视野，
      // 而"保存了没有"是用户点完按钮后唯一想知道的事。
      toast.success("设置已保存");
    } catch (requestError) {
      toast.error("保存失败", getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  const dirty =
    settings !== null && draft !== null && JSON.stringify(settings) !== JSON.stringify(draft);

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-xs text-muted-foreground">
          设置改动后需要重启网关才会对监听地址生效；任务相关设置即时生效。
        </span>
        <div className="ml-auto flex items-center gap-2">
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
            重新读取
          </Button>
          <Button size="sm" onClick={() => void save()} disabled={pending || !dirty}>
            {pending ? "保存中..." : "保存"}
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



      <MyBox isLoading={isLoading && settings === null} text="正在读取设置...">
        {draft ? (
          <div className="flex flex-col gap-3">
            <PageSection title="网关" description="监听地址与端口（重启生效）">
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <Field label="监听地址" hint="生产环境改 0.0.0.0 时必须配置密钥">
                  <input
                    value={draft.gatewayBind}
                    onChange={(event) => update("gatewayBind", event.target.value)}
                    className={cn(inputClass, "font-mono")}
                  />
                </Field>
                <Field label="端口">
                  <input
                    value={String(draft.gatewayPort)}
                    onChange={(event) =>
                      update("gatewayPort", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    className={cn(inputClass, "font-mono")}
                  />
                </Field>
              </div>
            </PageSection>

            <PageSection title="任务" description="执行节奏与记录保留">
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <Field
                  label="记录保留天数"
                  error={retentionError}
                  hint="超期记录每天清理一次，明细随记录级联删除"
                >
                  <input
                    value={String(draft.taskRetentionDays)}
                    onChange={(event) =>
                      update("taskRetentionDays", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    aria-invalid={retentionError ? true : undefined}
                    className={inputClass}
                  />
                </Field>

                <Field
                  label="定时巡检"
                  hint="默认开启：启动 20s 后首轮，之后每 60s 巡检"
                >
                  <div className="flex h-8 items-center gap-2">
                    <Switch
                      checked={draft.taskScheduleEnabled}
                      onCheckedChange={(checked) => update("taskScheduleEnabled", checked)}
                      aria-label="启用定时巡检"
                    />
                    <span className="text-xs text-muted-foreground">
                      {draft.taskScheduleEnabled ? "已开启" : "已关闭"}
                    </span>
                  </div>
                </Field>

                {/*
                  探活：主动发现"凭据过期/被封/额度耗尽"。
                  不探活的话，这类故障只有等真实用户请求失败才会暴露。
                */}
                <Field
                  label="定时探活"
                  hint="对每个启用的账号发一句 hello，确认凭据还能用。会消耗少量上游额度"
                >
                  <div className="flex h-8 items-center gap-2">
                    <Switch
                      checked={draft.probeEnabled}
                      onCheckedChange={(checked) => update("probeEnabled", checked)}
                      aria-label="启用定时探活"
                    />
                    <span className="text-xs text-muted-foreground">
                      {draft.probeEnabled ? "已开启" : "已关闭"}
                    </span>
                  </div>
                </Field>

                <Field
                  label="探活间隔（分钟）"
                  hint="最短 5 分钟。间隔越短发现越快，但消耗的上游额度也越多"
                >
                  <input
                    value={String(draft.probeIntervalMinutes)}
                    onChange={(event) =>
                      update("probeIntervalMinutes", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    className={inputClass}
                  />
                </Field>

                <Field label="账号间隔（ms）" hint="账号之间串行执行的等待时间">
                  <input
                    value={String(draft.taskPerAccountGapMs)}
                    onChange={(event) =>
                      update("taskPerAccountGapMs", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    className={inputClass}
                  />
                </Field>

                <Field label="动作间隔（ms）" hint="同一账号内动作之间的等待时间">
                  <input
                    value={String(draft.taskDailyGapMs)}
                    onChange={(event) =>
                      update("taskDailyGapMs", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    className={inputClass}
                  />
                </Field>

                <Field label="上报间隔（ms）" hint="任务事件上报之间的等待时间">
                  <input
                    value={String(draft.taskReportGapMs)}
                    onChange={(event) =>
                      update("taskReportGapMs", Number(event.target.value) || 0)
                    }
                    inputMode="numeric"
                    className={inputClass}
                  />
                </Field>
              </div>
            </PageSection>

            <PageSection title="日志" description="用量与错误原文的保留策略">
              <Field label="用量保留天数" hint="超期记录会被清理">
                <input
                  value={String(draft.logRetentionDays)}
                  onChange={(event) =>
                    update("logRetentionDays", Number(event.target.value) || 0)
                  }
                  inputMode="numeric"
                  className={cn(inputClass, "max-w-[12rem]")}
                />
              </Field>
            </PageSection>

            <PageSection
              title="通知"
              description="掉号 / 额度不足 / 错误率突增时推送到 Webhook（POST JSON）"
            >
              <Field
                label="Webhook 地址"
                error={webhookError}
                hint="留空则不发任何通知。同一告警 5 分钟内不重复推送。"
              >
                <input
                  value={draft.webhookUrl}
                  onChange={(event) => update("webhookUrl", event.target.value)}
                  placeholder="https://example.com/relayai-alert"
                  aria-invalid={webhookError ? true : undefined}
                  className={cn(inputClass, "font-mono text-xs")}
                />
              </Field>
            </PageSection>
          </div>
        ) : (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <SettingsIcon className="size-3.5" aria-hidden="true" />
            没有读到设置
          </div>
        )}
      </MyBox>
    </div>
  );
};
