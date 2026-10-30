import { useEffect, useMemo, useState } from "react";
import { Activity } from "lucide-react";
import { listCatalogModels, type CatalogModel } from "@/api/catalog";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

/**
 * 「分组状态」展示设置：是否对外展示 + 用哪些模型探活。
 *
 * 为什么把这两件事放在同一块：
 *   它们服务同一个目的 —— 「分组状态」页是**给人看的状态板**，
 *   展示哪个分组、用哪个模型证明它能用，是同一个决定的两面。
 *   早先探活模型设置在那张页面自己身上，于是"想把国际版藏起来"要先去
 *   分组状态页找到它、再点铅笔 —— 而藏起来之后那张卡片就没了入口。
 *
 * 为什么默认不展示（`statusVisible=false`）：
 *   分组是调度用的计价桶，建出来不等于要公开。展示是明确动作，
 *   否则内部试用的分组会直接出现在门面上。
 */
export type StatusSettingsValue = {
  /** 是否在「分组状态」页展示。 */
  visible: boolean;
  /** 定时探活要读的模型列表；空 = 不探（不会自动扩大范围）。 */
  probeModels: string[];
};

export const ChannelStatusSettings = ({
  channelId,
  value,
  disabled,
  onChange,
}: {
  /** 分组 id；新建流程里为空，此时还没拉过模型目录。 */
  channelId: string;
  value: StatusSettingsValue;
  disabled?: boolean;
  onChange: (next: StatusSettingsValue) => void;
}) => {
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!channelId) {
      setModels([]);
      return;
    }
    let active = true;
    setLoading(true);
    // 拉目录失败只让列表退化，不阻断表单：展示开关本身仍然能改。
    void listCatalogModels()
      .then((response) => {
        if (active) setModels(response.data?.models ?? []);
      })
      .catch(() => {
        if (active) setModels([]);
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [channelId]);

  /**
   * 只列**这个分组真的提供**的模型。
   *
   * 列全量会让人选到一个该分组根本没有的模型，探活必然失败 ——
   * 那种失败是配置错误，不是账号故障，会把排查方向带偏。
   */
  const options = useMemo(
    () =>
      models
        .filter(
          (model) =>
            model.enabled !== false &&
            model.channels.some((item) => item.id === channelId && item.exposed),
        )
        .map((model) => model.name)
        .sort((left, right) => left.localeCompare(right, "zh-CN")),
    [channelId, models],
  );

  const toggleModel = (name: string, checked: boolean) => {
    const next = checked
      ? [...value.probeModels, name]
      : value.probeModels.filter((item) => item !== name);
    onChange({ ...value, probeModels: [...new Set(next)] });
  };

  return (
    <div className="flex flex-col gap-3 rounded-lg border bg-muted/30 p-3">
      <div className="flex items-start gap-2">
        <Activity className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <p className="text-xs font-medium">分组状态</p>
          <p className="mt-0.5 text-[11px] leading-4 text-muted-foreground">
            开启后这个分组才会出现在「分组状态」页。默认不展示 ——
            只想公开国内版时，把国际版保持关闭即可。
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Switch
            checked={value.visible}
            disabled={disabled}
            onCheckedChange={(checked) => onChange({ ...value, visible: checked })}
            aria-label="在分组状态页展示"
          />
          <span
            className={cn(
              "text-[11px]",
              value.visible ? "text-success" : "text-muted-foreground",
            )}
          >
            {value.visible ? "展示中" : "不展示"}
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-1.5 border-t pt-2.5">
        <span className="text-xs font-medium">探活模型</span>
        {!channelId ? (
          <p className="text-[11px] leading-4 text-muted-foreground">
            分组保存后，这里会列出它对外提供的模型。探活会真实消耗上游额度，
            所以**没选就完全不探**。
          </p>
        ) : loading ? (
          <p className="text-[11px] text-muted-foreground">正在读取该分组的模型目录…</p>
        ) : options.length === 0 ? (
          <p className="text-[11px] leading-4 text-muted-foreground">
            这个分组还没有对外提供的模型，暂时无法设置探活模型。
            先给它添加账号，网关拉到模型目录后再回来设置。
          </p>
        ) : (
          <>
            <div className="flex flex-wrap gap-1.5">
              {options.map((name) => {
                const checked = value.probeModels.includes(name);
                return (
                  <label
                    key={name}
                    className={cn(
                      "inline-flex cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 font-mono text-[11px] transition-colors",
                      checked
                        ? "border-primary/40 bg-primary/10 text-foreground"
                        : "bg-card text-muted-foreground hover:border-border-strong",
                    )}
                  >
                    <input
                      type="checkbox"
                      className="size-3 accent-[var(--primary)]"
                      checked={checked}
                      disabled={disabled}
                      onChange={(event) => toggleModel(name, event.target.checked)}
                    />
                    {name}
                  </label>
                );
              })}
            </div>
            <p className="text-[11px] leading-4 text-muted-foreground">
              探活会用选中的模型各发一句 hello 来确认凭据可用，会真实消耗额度。
              一个都不选 = 这个分组不做定时探活。
            </p>
          </>
        )}
      </div>
    </div>
  );
};
