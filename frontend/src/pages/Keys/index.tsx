import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Ban,
  CheckCircle2,
  Copy,
  Download,
  KeyRound,
  PencilLine,
  Plus,
  RefreshCw,
  Search,
  Terminal,
  Trash2,
} from "lucide-react";
import { listCatalogModels, type CatalogModel } from "@/api/catalog";
import { listChannels, listModels } from "@/api/channels";
import { deleteKey, listKeys, updateKey } from "@/api/keys";
import type { ApiKey, Channel, Model } from "@/api/types";
import { ChannelPicker } from "@/components/common/ChannelPicker";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Pagination } from "@/components/common/Pagination";
import { RowActions } from "@/components/common/RowActions";
import { StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { KeyFormDialog } from "@/pages/Keys/components/KeyFormDialog";
import { getPublicConfig } from "@/api/system/publicConfig";
import { importToCcSwitch } from "@/lib/ccswitch";
import { toast } from "@/components/common/Toast/store";
import { compactNumber } from "@/lib/charts";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

/**
 * 密钥管理。
 *
 * 列与交互对齐 sub2api 的 `views/user/KeysView.vue`：
 *   · **API 密钥列**：脱敏显示 + 一键复制（明文存在库里，可反复复制）；
 *   · **渠道列**：显示绑定的渠道，未绑定 = "全部渠道"；
 *   · **操作列**：图标在上、文字在下的竖排按钮
 *     （使用密钥 / 导入 CCS / 禁用 / 编辑 / 删除），不是 `⋯` 菜单。
 *
 * 为什么"渠道"必须能选：一个 Key 绑一个渠道，才能做到
 * "便宜的走低价通道、重要的走稳定通道"。两个 allowlist 都留空 = 不限。
 */
export const KeysPage = () => {
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [models, setModels] = useState<Model[]>([]);
  /** 模型 × 分组归属：给 Key 表单按所选分组收窄模型选项。 */
  const [catalogModels, setCatalogModels] = useState<CatalogModel[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [keyword, setKeyword] = useState("");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ApiKey | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ApiKey | null>(null);
  const [useTarget, setUseTarget] = useState<ApiKey | null>(null);
  const [ccsTarget, setCcsTarget] = useState<ApiKey | null>(null);
  const [copied, setCopied] = useState<number | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState("");
  /**
   * 网关的对外地址（来自 `/public-config`）。
   *
   * 不能退回 `window.location.origin`：管理台在 5173、网关在 18788。
   * 拉不到时给空串，界面上显示"地址未知"而不是一个错的地址 ——
   * 错的地址比没有更糟，用户会照着填、然后以为网关坏了。
   */
  const [gatewayBaseUrl, setGatewayBaseUrl] = useState("");

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const [keyResponse, channelResponse, modelResponse, catalogResponse, configResponse] = await Promise.all([
        listKeys(),
        listChannels().catch(() => null),
        listModels().catch(() => null),
        listCatalogModels().catch(() => null),
        getPublicConfig().catch(() => null),
      ]);
      setKeys(keyResponse.data ?? []);
      setChannels(channelResponse?.data ?? []);
      setModels(modelResponse?.data ?? []);
      setCatalogModels(catalogResponse?.data?.models ?? []);
      setGatewayBaseUrl(configResponse?.data?.gatewayBaseUrl ?? "");
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

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    if (!trimmed) {
      return keys;
    }
    return keys.filter(
      (key) =>
        key.name.toLowerCase().includes(trimmed) ||
        key.keyPrefix.toLowerCase().includes(trimmed),
    );
  }, [keys, keyword]);

  useEffect(() => {
    setPage(1);
  }, [keyword]);

  const paged = useMemo(
    () => filtered.slice((page - 1) * pageSize, page * pageSize),
    [filtered, page, pageSize],
  );

  const copyKey = useCallback(async (key: ApiKey) => {
    if (!key.plaintext) {
      return;
    }
    try {
      await navigator.clipboard.writeText(key.plaintext);
      setCopied(key.id);
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      setCopied(null);
    }
  }, []);

  const toggleEnabled = useCallback(
    async (key: ApiKey) => {
      setBusy(`toggle-${key.id}`);
      setError("");
      try {
        await updateKey(key.id, { enabled: !key.enabled });
        setReloadKey((value) => value + 1);
      } catch (requestError) {
        setError(getRequestErrorMessage(requestError));
      } finally {
        setBusy("");
      }
    },
    [],
  );

  /**
   * 表格内直接切换渠道。
   *
   * 只提交 `channelAllow`：写成 `[id]` 或 `[]`（不绑定）。
   * 只发这一个字段而不是整行 —— 整行覆盖会把用户刚在别处改的
   * RPM/并发一起写回旧值。
   */
  const changeChannel = useCallback(async (key: ApiKey, channelID: string) => {
    setBusy(`channel-${key.id}`);
    setError("");
    try {
      await updateKey(key.id, { channelAllow: channelID ? [channelID] : [] });
      setReloadKey((value) => value + 1);
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setBusy("");
    }
  }, []);

  const channelOptions = useMemo(
    () =>
      channels.map((channel) => ({
        value: channel.id,
        label: channel.name,
        hint: `×${channel.channelRatio.toFixed(2)}`,
      })),
    [channels],
  );

  const columns: Array<Column<ApiKey>> = useMemo(
    () => [
      {
        key: "name",
        header: "名称",
        width: "160px",
        render: (key) => (
          <span className="block truncate font-medium" title={key.name}>
            {key.name}
          </span>
        ),
      },
      {
        key: "secret",
        header: "API 密钥",
        width: "220px",
        render: (key) => (
          <div className="flex items-center gap-1.5">
            <code className="truncate rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
              {key.plaintext
                ? `${key.plaintext.slice(0, 6)}...${key.plaintext.slice(-4)}`
                : key.keyPrefix}
            </code>
            <Button
              variant="ghost"
              size="icon-xs"
              disabled={!key.plaintext}
              onClick={(event) => {
                event.stopPropagation();
                void copyKey(key);
              }}
              aria-label={key.plaintext ? "复制密钥" : "明文已丢失，请轮换"}
              title={key.plaintext ? "复制密钥" : "明文已丢失，请轮换"}
              className={cn(copied === key.id ? "text-success" : "text-muted-foreground")}
            >
              {copied === key.id ? (
                <CheckCircle2 aria-hidden="true" />
              ) : (
                <Copy aria-hidden="true" />
              )}
            </Button>
          </div>
        ),
      },
      {
        key: "channel",
        header: "渠道",
        width: "220px",
        // 表格内**可直接切换渠道**（对齐 sub2api 的 #cell-group）：
        // 换渠道是高频操作（试哪个通道更便宜/更稳），每次开弹窗太慢。
        //
        // **没有"全部渠道"这个概念**：一把密钥必须绑一个渠道。
        // 空值只可能是历史数据（早期多选时代的遗留），显示成
        // "请选择渠道"并让用户补上，而不是把它当成一种合法状态。
        render: (key) => (
          <ChannelPicker
            value={key.channelAllow[0] ?? ""}
            options={channelOptions}
            placeholder="请选择渠道"
            disabled={busy === `channel-${key.id}`}
            onChange={(next) => void changeChannel(key, next)}
            ariaLabel={`切换 ${key.name} 的渠道`}
          />
        ),
      },
      {
        key: "models",
        header: "模型",
        width: "92px",
        hideOnMobile: true,
        render: (key) => (
          <span className="text-xs text-muted-foreground">
            {key.modelAllow.length === 0 ? "全部" : `${key.modelAllow.length} 个`}
          </span>
        ),
      },
      {
        key: "usage",
        header: "用量",
        width: "150px",
        hideOnMobile: true,
        render: (key) => {
          const usage = key.usage;
          return (
            <div className="flex flex-col gap-0.5 text-[11px] leading-4 tabular-nums">
              <span title="累计 token">
                {compactNumber(usage?.totalTokens ?? 0)} token
              </span>
              <span className="text-muted-foreground">
                今日 {compactNumber(usage?.todayTokens ?? 0)}
              </span>
            </div>
          );
        },
      },
      {
        key: "cost",
        header: "消费",
        width: "128px",
        hideOnMobile: true,
        render: (key) => {
          const usage = key.usage;
          return (
            <div className="flex flex-col gap-0.5 text-[11px] leading-4 tabular-nums">
              <span className={cn((usage?.totalCost ?? 0) > 0 && "text-foreground")}>
                ¥{(usage?.totalCost ?? 0).toFixed(4)}
              </span>
              <span className="text-muted-foreground">
                今日 ¥{(usage?.todayCost ?? 0).toFixed(4)}
              </span>
            </div>
          );
        },
      },
      {
        key: "limits",
        header: "限制",
        width: "132px",
        hideOnMobile: true,
        render: (key) => {
          const parts = [
            key.rpmLimit ? `${key.rpmLimit} RPM` : null,
            key.maxConcurrency ? `并发 ${key.maxConcurrency}` : null,
            key.dailyTokenLimit ? `${key.dailyTokenLimit} token/日` : null,
          ].filter(Boolean);
          return (
            <span className="block truncate text-xs text-muted-foreground">
              {parts.length === 0 ? "不限" : parts.join(" · ")}
            </span>
          );
        },
      },
      {
        key: "lastUsed",
        header: "最近使用",
        width: "112px",
        hideOnMobile: true,
        render: (key) => (
          <span className="text-xs tabular-nums text-muted-foreground">
            {key.lastUsedAt
              ? new Date(key.lastUsedAt).toLocaleString("zh-CN", {
                  month: "2-digit",
                  day: "2-digit",
                  hour: "2-digit",
                  minute: "2-digit",
                })
              : "从未"}
          </span>
        ),
      },
      {
        key: "status",
        header: "状态",
        width: "80px",
        render: (key) => (
          <StatusBadge
            tone={key.enabled ? "success" : "neutral"}
            label={key.enabled ? "启用" : "停用"}
          />
        ),
      },
      {
        key: "actions",
        header: "操作",
        width: "260px",
        align: "center",
        render: (key) => (
          <RowActions
            actions={[
              {
                // 没有明文的 Key（迁移前建的）无法"使用"：拿不到 sk- 就拼不出 curl。
                // 直接置灰而不是点了才报错。
                label: key.plaintext ? "使用密钥" : "明文已丢失，请轮换",
                id: "use",
                icon: Terminal,
                tone: "success",
                disabled: !key.plaintext,
                onClick: () => setUseTarget(key),
              },
              {
                label: key.plaintext ? "导入 CC Switch" : "明文已丢失，请轮换",
                id: "ccs",
                icon: Download,
                tone: "primary",
                disabled: !key.plaintext,
                // 不直接导入：先让用户选客户端类型。
                // CC Switch 里 Claude / Gemini 是两套不同的配置格式，
                // 猜错了会往错误的客户端里塞配置。
                onClick: () => setCcsTarget(key),
              },
              {
                label: key.enabled ? "禁用" : "启用",
                id: "toggle",
                icon: key.enabled ? Ban : CheckCircle2,
                tone: "warning",
                disabled: busy === `toggle-${key.id}`,
                onClick: () => void toggleEnabled(key),
              },
              {
                label: "编辑",
                id: "edit",
                icon: PencilLine,
                onClick: () => {
                  setEditing(key);
                  setFormOpen(true);
                },
              },
              {
                label: "删除",
                id: "delete",
                icon: Trash2,
                tone: "danger",
                onClick: () => setDeleteTarget(key),
              },
            ]}
          />
        ),
      },
    ],
    [busy, changeChannel, channelOptions, copied, copyKey, toggleEnabled],
  );

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-3">
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
            placeholder="搜索名称或 Key"
            aria-label="搜索密钥"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

        {selected.size > 0 ? (
          <Button
            variant="outline"
            size="sm"
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            onClick={() => {
              const ids = [...selected];
              setSelected(new Set());
              void Promise.all(ids.map((key) => deleteKey(Number(key))))
                .then(() => setReloadKey((value) => value + 1))
                .catch((requestError) => setError(getRequestErrorMessage(requestError)));
            }}
          >
            <Trash2 aria-hidden="true" />
            删除所选（{selected.size}）
          </Button>
        ) : null}

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
            刷新
          </Button>
          <Button
            size="sm"
            onClick={() => {
              setEditing(null);
              setFormOpen(true);
            }}
          >
            <Plus aria-hidden="true" />
            新建密钥
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

      <MyBox
        isLoading={isLoading && keys.length === 0}
        text="正在加载密钥..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={paged}
          rowKey={(key) => String(key.id)}
          selection={{
            selected,
            onChange: setSelected,
            label: (key) => `选择密钥 ${key.name}`,
          }}
          empty={
            keys.length === 0 ? (
              <EmptyState
                icon={KeyRound}
                title="还没有密钥"
                description="客户端需要 sk- 开头的密钥才能调用网关。建议按客户端分别创建，方便单独吊销。"
                action={
                  <Button
                    size="sm"
                    onClick={() => {
                      setEditing(null);
                      setFormOpen(true);
                    }}
                  >
                    <Plus aria-hidden="true" />
                    新建密钥
                  </Button>
                }
              />
            ) : (
              <EmptyState
                icon={Search}
                title="没有匹配的密钥"
                description={`没有名称或前缀包含「${keyword}」的密钥。`}
                action={
                  <Button variant="outline" size="sm" onClick={() => setKeyword("")}>
                    清除搜索
                  </Button>
                }
              />
            )
          }
        />
      </MyBox>

      <Pagination
        page={page}
        pageSize={pageSize}
        total={filtered.length}
        onPageChange={setPage}
        onPageSizeChange={(size) => {
          setPageSize(size);
          setPage(1);
        }}
        disabled={isLoading}
      />

      <KeyFormDialog
        open={formOpen}
        apiKey={editing}
        channels={channels}
        models={models}
        catalogModels={catalogModels}
        onOpenChange={setFormOpen}
        onSaved={() => {
          setFormOpen(false);
          setReloadKey((value) => value + 1);
        }}
      />

      {/* 使用密钥：给出可直接粘贴的 curl 与 base_url */}
      <Dialog open={useTarget !== null} onOpenChange={(next) => !next && setUseTarget(null)}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle>使用密钥 · {useTarget?.name}</DialogTitle>
            <DialogDescription>
              把下面任一段贴进客户端即可。密钥完整值在「API 密钥」列可复制。
            </DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium">Base URL</span>
              <code className="rounded bg-muted px-2 py-1.5 font-mono text-xs">
                {gatewayBaseUrl ? `${gatewayBaseUrl}/v1` : "地址未知（后端未配置 GATEWAY_BASE_URL）"}
              </code>
            </div>
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium">curl</span>
              <pre className="overflow-auto rounded bg-muted px-2 py-1.5 font-mono text-[11px] whitespace-pre-wrap">
                {`curl ${gatewayBaseUrl}/v1/chat/completions \\
  -H "Authorization: Bearer ${useTarget?.plaintext ?? "<请轮换后获取>"}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'`}
              </pre>
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setUseTarget(null)}>
              关闭
            </Button>
            <Button
              size="sm"
              onClick={() => {
                if (useTarget?.plaintext) {
                  void copyKey(useTarget);
                }
              }}
              disabled={!useTarget?.plaintext}
            >
              <Copy aria-hidden="true" />
              复制密钥
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* CC Switch 导入：先选客户端类型，再唤起协议 */}
      <Dialog
        open={ccsTarget !== null}
        onOpenChange={(next) => !next && setCcsTarget(null)}
      >
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>导入到 CC Switch</DialogTitle>
            <DialogDescription>
              选择要导入到哪个客户端。CC Switch 会收到一个
              <code className="mx-1 rounded bg-muted px-1 font-mono text-[11px]">
                ccswitch://
              </code>
              链接并自动建好 provider。
            </DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3">
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium">网关地址</span>
              <code className="rounded bg-muted px-2 py-1.5 font-mono text-xs">
                {gatewayBaseUrl || "地址未知（后端未配置 GATEWAY_BASE_URL）"}
              </code>
              <p className="text-[11px] text-muted-foreground">
                这是网关的地址，不是当前管理台的地址。两者是不同的进程。
              </p>
            </div>

            <div className="grid grid-cols-2 gap-2">
              {(
                [
                  { type: "claude" as const, label: "Claude Code", hint: "app=claude" },
                  { type: "gemini" as const, label: "Gemini CLI", hint: "app=gemini" },
                ]
              ).map((option) => (
                <Button
                  key={option.type}
                  variant="outline"
                  size="sm"
                  disabled={!gatewayBaseUrl}
                  onClick={() => {
                    const target = ccsTarget;
                    setCcsTarget(null);
                    if (!target) {
                      return;
                    }
                    const ok = importToCcSwitch({
                      baseUrl: gatewayBaseUrl,
                      providerName: "relayAI",
                      apiKey: target.plaintext ?? "",
                      clientType: option.type,
                    });
                    if (ok) {
                      toast.success("已发送到 CC Switch", "请在客户端里确认新增的 provider");
                    } else {
                      toast.info(
                        "已尝试唤起 CC Switch",
                        "若没有反应，说明本机没有安装或未注册 ccswitch:// 协议",
                      );
                    }
                  }}
                  className="flex h-auto flex-col items-start gap-0.5 py-2"
                >
                  <span className="text-xs font-medium">{option.label}</span>
                  <span className="text-[11px] text-muted-foreground">{option.hint}</span>
                </Button>
              ))}
            </div>

            {!gatewayBaseUrl ? (
              <p className="rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-[11px] text-warning">
                拿不到网关地址，无法生成导入链接。请确认后端设置了 GATEWAY_BASE_URL。
              </p>
            ) : null}
          </div>

          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setCcsTarget(null)}>
              取消
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 删除确认 */}
      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(next) => !next && setDeleteTarget(null)}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>删除密钥</DialogTitle>
            <DialogDescription>
              删除后使用「{deleteTarget?.name}」的客户端会立即失效，且无法恢复。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setDeleteTarget(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                const target = deleteTarget;
                setDeleteTarget(null);
                if (!target) {
                  return;
                }
                void deleteKey(target.id)
                  .then(() => setReloadKey((value) => value + 1))
                  .catch((requestError) => setError(getRequestErrorMessage(requestError)));
              }}
            >
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

    </div>
  );
};
