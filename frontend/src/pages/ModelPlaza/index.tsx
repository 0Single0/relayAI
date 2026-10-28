import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Boxes,
  Coins,
  Download,
  Layers,
  PencilLine,
  RefreshCw,
  Search,
  SlidersHorizontal,
} from "lucide-react";
import { Link, useLocation } from "react-router-dom";
import {
  applyBuiltinPrices,
  applyOpenRouterPrices,
  listCatalogModels,
  syncCatalog,
  type CatalogModel,
} from "@/api/catalog";
import { listChannels } from "@/api/channels";
import { getPublicModels } from "@/api/public";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { ModelLogo } from "@/components/common/ModelLogo";
import { toast } from "@/components/common/Toast/store";
import {
  capabilitySource,
  capabilitySourceNote,
  knownCapabilities,
} from "@/components/common/ModelLogo/capabilities";
import { vendorOf } from "@/components/common/ModelLogo/vendor";
import { ModelPriceDialog } from "@/pages/ModelPlaza/components/ModelPriceDialog";
import { MyBox } from "@/components/common/MyBox";
import { StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { useAuthStore } from "@/store/auth";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const ALL = "__all__";

const compact = (value: number) => {
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(0)}M`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(0)}K`;
  }
  return String(value);
};

/**
 * 价格展示：元 / 百万 token。
 *
 * 0 显示"未定价"而不是 `¥0.00`：后者会被读成"免费"，
 * 那会让用户以为用这个模型不花钱 —— 而实际情况是**没配价、成本算成 0**。
 */
const priceText = (value: number) => (value > 0 ? `¥${value.toFixed(2)}` : "未定价");

const priceTone = (value: number) =>
  value > 0 ? "text-foreground" : "text-warning";

/**
 * 模型广场。
 *
 * 两个视角：
 *   · **按渠道分组**（管理面）：同一个模型在不同渠道的价格/可用性不同，
 *     平铺成一张表看不出这件事。分组后能回答"这个模型走哪个渠道最便宜"。
 *   · **公开清单**（未登录）：只给模型与最低价，不给渠道归属 ——
 *     渠道是部署细节，对外没有价值。
 *
 * 定价来源三种，页面顶部都给了入口：
 *   1. 内置价格表（离线可用，会过时）；
 *   2. OpenRouter 实时拉取（准，但没有国内自建模型）；
 *   3. 手工逐个改（最终兜底）。
 */
export const ModelPlazaPage = () => {
  const user = useAuthStore((state) => state.user);
  const location = useLocation();
  const standalone = location.pathname === "/plaza";

  const [models, setModels] = useState<CatalogModel[]>([]);
  /** 分组列表：用于"哪些分组一个模型都没有"的提示与筛选。 */
  const [channels, setChannels] = useState<Array<{ id: string; name: string }>>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [keyword, setKeyword] = useState("");
  const [channelFilter, setChannelFilter] = useState(ALL);
  const [reloadKey, setReloadKey] = useState(0);
  /** 正在编辑价格的模型；null = 对话框关闭。 */
  const [priceTarget, setPriceTarget] = useState<CatalogModel | null>(null);

  const isAdmin = user?.role === "admin";

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      if (isAdmin) {
        const [response, channelResponse] = await Promise.all([
          listCatalogModels(),
          listChannels().catch(() => null),
        ]);
        setModels(response.data?.models ?? []);
        setChannels(channelResponse?.data ?? []);
      } else {
        // 非管理员（含未登录）：走公开端点，拿不到渠道归属。
        const response = await getPublicModels();
        setModels(
          (response.data?.models ?? []).map((item) => ({
            name: item.name,
            displayName: item.displayName,
            contextWindow: item.contextWindow,
            maxOutput: item.maxOutput,
            capabilities: item.capabilities,
            inputPrice: item.inputPrice,
            outputPrice: item.outputPrice,
            cacheReadPrice: item.cacheReadPrice ?? 0,
            modelRatio: 1,
            enabled: true,
            priced: item.priced ?? (item.inputPrice > 0 || item.outputPrice > 0),
            channels: [],
          })),
        );
      }
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }
  }, [isAdmin]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const runAction = useCallback(
    async (key: string, action: () => Promise<string>) => {
      setBusy(key);
      try {
        toast.success("操作成功", await action());
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        toast.error("操作失败", getRequestErrorMessage(requestError));
      } finally {
        setBusy("");
      }
    },
    [],
  );

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    return models.filter((model) => {
      if (channelFilter !== ALL) {
        if (!model.channels.some((channel) => channel.id === channelFilter)) {
          return false;
        }
      }
      if (!trimmed) {
        return true;
      }
      return (
        model.name.toLowerCase().includes(trimmed) ||
        model.displayName.toLowerCase().includes(trimmed)
      );
    });
  }, [channelFilter, keyword, models]);

  /**
   * 按渠道分组：key 为渠道 id，`ALL` 表示"没有分组对外提供"。
   *
   * **只统计 `exposed` 的分组**：被白名单挡住的分组不算"提供这个模型"，
   * 否则把 WorkBuddy 限制成只用 ds/glm 之后，其余模型仍会挂在它下面 ——
   * 与"模型广场只剩那几个"的需求正好相反。
   */
  const grouped = useMemo(() => {
    const map = new Map<string, { name: string; ratio: number; models: CatalogModel[] }>();

    for (const model of filtered) {
      const exposedChannels = model.channels.filter((channel) => channel.exposed);

      // 非管理员（含未登录）：公开端点已经过滤过了，channels 为空只代表
      // "没有渠道归属可展示"，不代表"未开放"—— 按已开放处理。
      if (!isAdmin && exposedChannels.length === 0) {
        const entry = map.get(ALL) ?? { name: "全部模型", ratio: 1, models: [] };
        entry.models.push(model);
        map.set(ALL, entry);
        continue;
      }

      if (exposedChannels.length === 0) {
        // 一个模型没有任何分组对外提供 → 不该出现在广场里。
        //
        // 但**管理员**需要看到它：这正是"我配了白名单、结果它消失了"时的排查依据。
        // 所以按角色分流：管理员归到一个"未对外开放"分组，普通用户直接略过。
        if (!isAdmin) {
          continue;
        }
        const entry = map.get(ALL) ?? { name: "未对外开放", ratio: 1, models: [] };
        entry.models.push(model);
        map.set(ALL, entry);
        continue;
      }
      for (const channel of exposedChannels) {
        const entry = map.get(channel.id) ?? {
          name: channel.name,
          ratio: channel.ratio,
          models: [],
        };
        entry.models.push(model);
        map.set(channel.id, entry);
      }
    }

    // 把**没有出现在任何模型里**的分组也列出来。
    //
    // 为什么：分组名下账号全停用时，网关不拉它的模型目录，
    // 于是它一个模型都没有。只从模型反推的话它根本不会出现，
    // 管理员会以为"分组丢了"，而真实原因是"没有可用账号"。
    if (isAdmin) {
      for (const channel of channels) {
        if (!map.has(channel.id)) {
          map.set(channel.id, { name: channel.name, ratio: 1, models: [] });
        }
      }
    }

    // 渠道按名称排，`ALL` 兜底放最后。
    return [...map.entries()].sort(([left], [right]) => {
      if (left === ALL) {
        return 1;
      }
      if (right === ALL) {
        return -1;
      }
      return left.localeCompare(right);
    });
  }, [channels, filtered, isAdmin]);

  const unpriced = useMemo(() => models.filter((model) => !model.priced).length, [models]);

  /**
   * 渠道筛选选项。
   *
   * **来自分组表而不是从模型反推**：一个分组可能一个模型都没有
   * （名下账号全停用 → 网关不拉它的目录）。反推的话它根本不会出现在筛选里，
   * 管理员就看不到"这个分组为什么没模型"。
   */
  const channelOptions = useMemo(() => {
    const map = new Map<string, string>();
    for (const channel of channels) {
      map.set(channel.id, channel.name);
    }
    // 补上模型里出现、但分组表里没有的（降级情况）。
    for (const model of models) {
      for (const channel of model.channels) {
        if (!map.has(channel.id)) {
          map.set(channel.id, channel.name);
        }
      }
    }
    return [...map.entries()];
  }, [channels, models]);

  const pageClass = cn(
    "flex min-w-0 flex-col gap-3",
    standalone
      ? "mx-auto min-h-dvh w-full max-w-[1400px] px-4 py-4 sm:px-6 md:px-8 md:py-6 lg:px-10"
      : "min-h-0 flex-1",
  );

  return (
    <div className={pageClass}>
      {standalone ? (
        <div className="flex flex-col gap-0.5">
          <h1 className="text-base font-medium">模型广场</h1>
          <p className="text-xs text-muted-foreground">
            可用模型、上下文与价格；价格按渠道倍率折算
          </p>
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 sm:max-w-xs">
          <Search
            className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <input
            type="search"
            value={keyword}
            onChange={(event) => setKeyword(event.target.value)}
            placeholder="搜索模型"
            aria-label="搜索模型"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

        {isAdmin && channelOptions.length > 0 ? (
          <div className="flex flex-wrap items-center gap-1">
            <button
              type="button"
              onClick={() => setChannelFilter(ALL)}
              aria-pressed={channelFilter === ALL}
              className={cn(
                "cursor-pointer rounded-md border px-2 py-1 text-xs transition-colors",
                channelFilter === ALL
                  ? "border-primary bg-primary/10 text-primary"
                  : "text-muted-foreground hover:bg-muted",
              )}
            >
              全部渠道
            </button>
            {channelOptions.map(([id, name]) => (
              <button
                key={id}
                type="button"
                onClick={() => setChannelFilter(id)}
                aria-pressed={channelFilter === id}
                className={cn(
                  "cursor-pointer rounded-md border px-2 py-1 text-xs transition-colors",
                  channelFilter === id
                    ? "border-primary bg-primary/10 text-primary"
                    : "text-muted-foreground hover:bg-muted",
                )}
              >
                {name}
              </button>
            ))}
          </div>
        ) : null}

        <div className="ml-auto flex flex-wrap items-center gap-2">
          {isAdmin ? (
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  void runAction("sync", async () => {
                    const response = await syncCatalog();
                    const data = response.data;
                    return `已同步 ${data?.synced ?? 0} 个模型（清理 ${data?.removed ?? 0} 个）`;
                  })
                }
                disabled={busy !== ""}
              >
                <RefreshCw aria-hidden="true" />
                从渠道同步
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  void runAction("builtin", async () => {
                    const response = await applyBuiltinPrices();
                    return response.data?.message ?? "已填入内置价格";
                  })
                }
                disabled={busy !== ""}
              >
                <Coins aria-hidden="true" />
                内置价填充
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() =>
                  void runAction("openrouter", async () => {
                    const response = await applyOpenRouterPrices();
                    return response.data?.message ?? "已更新 OpenRouter 价格";
                  })
                }
                disabled={busy !== ""}
              >
                <Download aria-hidden="true" />
                拉 OpenRouter
              </Button>
            </>
          ) : null}
          <Button variant="outline" size="sm" onClick={() => setReloadKey((v) => v + 1)}>
            <RefreshCw aria-hidden="true" />
            刷新
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

      {isAdmin ? (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span>
            共 <span className="font-medium text-foreground">{models.length}</span> 个模型
          </span>
          <span className="text-border">·</span>
          <span>
            <span className="font-medium text-foreground">{grouped.length}</span> 个分组
          </span>
          {unpriced > 0 ? (
            <>
              <span className="text-border">·</span>
              <span className="text-warning">
                {unpriced} 个未定价（这些模型的成本会算成 0）
              </span>
            </>
          ) : null}
        </div>
      ) : null}

      <MyBox
        isLoading={isLoading && models.length === 0}
        text="正在加载模型目录..."
        className="flex min-h-0 flex-1 flex-col"
      >
        {filtered.length === 0 ? (
          <EmptyState
            icon={Boxes}
            title={models.length === 0 ? "还没有模型" : "没有匹配的模型"}
            description={
              models.length === 0
                ? isAdmin
                  ? "点「从渠道同步」把网关的模型目录拉过来。"
                  : "管理员还没有同步模型目录。"
                : `没有名称包含「${keyword}」的模型。`
            }
            action={
              models.length === 0 && isAdmin ? (
                <Button
                  size="sm"
                  onClick={() =>
                    void runAction("sync", async () => {
                      const response = await syncCatalog();
                      return `已同步 ${response.data?.synced ?? 0} 个模型`;
                    })
                  }
                  disabled={busy !== ""}
                >
                  <RefreshCw aria-hidden="true" />
                  从渠道同步
                </Button>
              ) : undefined
            }
          />
        ) : (
          <div className="flex flex-col gap-4">
            {grouped.map(([channelID, group]) => (
              <section key={channelID} className="flex flex-col gap-3">
                {/* 分组标题：渠道名 + 倍率 */}
                <div className="flex flex-wrap items-center gap-2">
                  <Layers className="size-3.5 text-muted-foreground" aria-hidden="true" />
                  <h2 className="text-sm font-medium">{group.name}</h2>
                  {channelID !== ALL ? (
                    <StatusBadge
                      tone="neutral"
                      label={`×${group.ratio.toFixed(2)}`}
                      dot={false}
                    />
                  ) : null}
                  <span className="text-[11px] text-muted-foreground">
                    {group.models.length} 个模型
                  </span>
                  {group.models.length === 0 ? (
                    <span className="text-[11px] text-warning">
                      该分组名下没有启用的账号，网关不会拉它的模型目录
                    </span>
                  ) : null}
                </div>

                {/*
                  最多一行 4 个：1536px（2xl）以下仍是 3 列。
                  再往上加列会让卡片宽过 400px，而内容只有两三行短文本，
                  横向留白堆积反而显得空。
                */}
                <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
                  {group.models.map((model) => (
                    <ModelCard
                      key={`${channelID}-${model.name}`}
                      model={model}
                      ratio={channelID === ALL ? 1 : group.ratio}
                      showChannel={false}
                      onEditPrice={isAdmin ? setPriceTarget : undefined}
                    />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
      </MyBox>

      {!user && !standalone ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>想用这些模型？</span>
          <Link to="/login" className="text-primary hover:underline">
            登录
          </Link>
        </div>
      ) : null}

      {/* 价格编辑：只有管理员能打开（普通用户的 onEditPrice 是 undefined）。 */}
      {isAdmin ? (
        <ModelPriceDialog
          model={priceTarget}
          onClose={() => setPriceTarget(null)}
          onSaved={() => {
            setPriceTarget(null);
            toast.success("价格已更新", "下一条请求即按新价计费");
            setReloadKey((value) => value + 1);
          }}
        />
      ) : null}
    </div>
  );
};

/**
 * 一个模型卡片。
 *
 * 三档价格分列而不是合成一个"均价"：缓存读取常常只有输入价的 1/10，
 * 而长对话里缓存能占输入的 80% 以上 —— 合成一个数就没法估算真实开销。
 */
const ModelCard = ({
  model,
  ratio,
  showChannel,
  onEditPrice,
}: {
  model: CatalogModel;
  ratio: number;
  showChannel: boolean;
  /** 仅管理员传入；普通用户为 undefined（不显示编辑入口）。 */
  onEditPrice?: (model: CatalogModel) => void;
}) => {
  // 能力用中文 + 图标展示，不直接显示 tools/vision/reasoning 这些字段名。
  const caps = knownCapabilities(model.capabilities);
  // 能力的来源决定"要不要额外说明"：实时声明不必解释，跨对照表/未知才要。
  const sourceNote = capabilitySourceNote(capabilitySource(model.capabilities));
  const vendor = vendorOf(model.name);

  return (
    /*
      `h-full`：让同一行的卡片等高。
      没有它时，少一个能力标签的卡片会比旁边矮一截，价格行跟着错位 ——
      那比"内容挤"更显乱。

      一列 4 个时卡片变窄，内容更容易折行；多列布局下用 `min-h-52`（208px）
      把"最矮的那张"垫到一个统一高度，避免有的卡片三行内容、有的四行，
      阴影一出来就看出高低不齐。
      单列（手机）不加这个下限：那里没有并排比较的对象，垫高只会在
      能力行和价格行之间留一条看得见的空带。
      再往上加就会让空带变得明显 —— 实测 208px 时那点留白刚好，
      240px 起就开始像"内容没填满"。
    */
    <article
      className={cn(
        "flex h-full flex-col gap-4 rounded-lg border bg-card p-5 md:min-h-52",
        // 浅阴影：只让卡片离开背景一点点，不做浮起的卡片墙。
        "shadow-[var(--shadow-card)] transition-shadow hover:shadow-[var(--shadow-card-hover)]",
      )}
    >
      <div className="flex items-start gap-2.5">
        <ModelLogo model={model.name} />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-medium" title={model.name}>
            {model.displayName || model.name}
          </h3>
          <p className="truncate font-mono text-[11px] text-muted-foreground">
            {vendor.label} · {model.name}
          </p>
        </div>
        {!model.priced ? (
          <StatusBadge tone="warning" label="未定价" dot={false} />
        ) : null}
        {onEditPrice ? (
          <button
            type="button"
            onClick={() => onEditPrice(model)}
            aria-label={`编辑 ${model.name} 的价格`}
            title="编辑价格"
            className={cn(
              "inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded",
              "text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
              "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
            )}
          >
            <PencilLine className="size-3.5" aria-hidden="true" />
          </button>
        ) : null}
      </div>

      {/*
        能力行下沉到价格区上方（`mt-auto`），而不是紧贴标题。
        原先它挨着模型名，视觉上像标题的第二行；一列 4 个、卡片变高之后
        标题下方那块空白会把它顶到视觉中心，反而比价格更抢眼。
        现在整张卡是"标题在上、能力+价格贴底"两段，扫视顺序更自然。
      */}
      <div className="mt-auto flex flex-col gap-1">
        <div className="flex flex-wrap items-center gap-1.5">
          {model.contextWindow ? (
            <span className="inline-flex items-center rounded-md bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
              {compact(model.contextWindow)} 上下文
            </span>
          ) : null}
          {caps.map((cap) => {
            const Icon = cap.icon;
            return (
              <span
                key={cap.key}
                title={cap.hint}
                className="inline-flex items-center gap-1 rounded-md bg-info/10 px-2 py-0.5 text-[11px] text-info"
              >
                <Icon className="size-3" aria-hidden="true" />
                {cap.label}
              </span>
            );
          })}
        </div>

        {/*
          能力来源做成**脚注**而不是又一个标签。
          它和上面的能力标签是两种东西（元信息 vs 能力本身）：做成同款胶囊时，
          「能力未知」看起来就像第四个能力，用户会以为"支持工具"和"能力未知"
          互相矛盾。降成一行小字之后，"这是对上面那几个标签的补充说明"才成立。
        */}
        {sourceNote ? (
          <p
            title={sourceNote.hint}
            className="truncate text-[11px] text-muted-foreground/80"
          >
            {sourceNote.label}
          </p>
        ) : null}
      </div>

      {/*
        价格区没有上边框：能力行与它是同一块信息的上下游，
        中间那条线既割裂了内容，又让本就不高的卡片显得更碎。
        用间距（上方 `gap-4`）区分层级就够了。
      */}
      <dl className="grid grid-cols-3 gap-3 text-xs">
        <div className="flex flex-col gap-0.5">
          <dt className="text-[11px] text-muted-foreground">输入 / 百万</dt>
          <dd className={cn("font-medium tabular-nums", priceTone(model.inputPrice))}>
            {priceText(model.inputPrice * ratio)}
          </dd>
        </div>
        <div className="flex flex-col gap-0.5">
          <dt className="text-[11px] text-muted-foreground">输出 / 百万</dt>
          <dd className={cn("font-medium tabular-nums", priceTone(model.outputPrice))}>
            {priceText(model.outputPrice * ratio)}
          </dd>
        </div>
        <div className="flex flex-col gap-0.5">
          <dt className="text-[11px] text-muted-foreground">缓存读</dt>
          <dd className={cn("font-medium tabular-nums", priceTone(model.cacheReadPrice))}>
            {priceText(model.cacheReadPrice * ratio)}
          </dd>
        </div>
      </dl>

      {showChannel && model.channels.length > 0 ? (
        <div className="flex flex-wrap items-center gap-1 border-t pt-2">
          <SlidersHorizontal className="size-3 text-muted-foreground" aria-hidden="true" />
          {model.channels.map((channel) => (
            <StatusBadge
              key={channel.id}
              // 未开放的分组标黄：让人一眼看出"这个号能跑、但分组白名单没放它出去"。
              tone={channel.exposed ? "neutral" : "warning"}
              label={`${channel.name} ×${channel.ratio.toFixed(2)}${
                channel.exposed ? "" : " · 未开放"
              }`}
              dot={false}
            />
          ))}
        </div>
      ) : null}
    </article>
  );
};
