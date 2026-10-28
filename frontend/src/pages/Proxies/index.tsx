import { useCallback, useEffect, useMemo, useState } from "react";
import { Network, PencilLine, Plus, RefreshCw, Search, Trash2 } from "lucide-react";
import {
  createProxy,
  deleteProxy,
  listProxies,
  updateProxy,
  type Proxy,
  type ProxyPayload,
} from "@/api/proxies";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

/**
 * 出站代理管理。
 *
 * 上游按出口 IP 限流与封禁 —— 所有账号共用服务器那一个 IP 时，
 * 几十个号的流量会叠在一起，很容易被判定成异常流量，
 * 表现为"偶发 429/403"，排查时又很容易误以为是凭据或倍率问题。
 *
 * 每个账号绑一个代理，就能把出口 IP 摊开。绑定在「账号管理」的行里改。
 */
export const ProxiesPage = () => {
  const [proxies, setProxies] = useState<Proxy[]>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const [keyword, setKeyword] = useState("");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<Proxy | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Proxy | null>(null);

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const response = await listProxies();
      setProxies(response.data ?? []);
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
      return proxies;
    }
    return proxies.filter(
      (proxy) =>
        proxy.name.toLowerCase().includes(trimmed) ||
        proxy.host.toLowerCase().includes(trimmed),
    );
  }, [keyword, proxies]);

  useEffect(() => {
    setPage(1);
  }, [keyword]);

  const paged = useMemo(
    () => filtered.slice((page - 1) * pageSize, page * pageSize),
    [filtered, page, pageSize],
  );

  const columns: Array<Column<Proxy>> = useMemo(
    () => [
      {
        key: "name",
        header: "名称",
        width: "18%",
        bodyAlign: "left",
        render: (proxy) => (
          <div className="min-w-0">
            <p className="truncate font-medium">{proxy.name}</p>
            {proxy.note ? (
              <p className="truncate text-[11px] text-muted-foreground">{proxy.note}</p>
            ) : null}
          </div>
        ),
      },
      {
        key: "protocol",
        header: "协议",
        width: "88px",
        render: (proxy) => (
          <StatusBadge tone="neutral" label={proxy.protocol} dot={false} />
        ),
      },
      {
        key: "address",
        header: "地址",
        width: "220px",
        render: (proxy) => (
          <span className="block truncate font-mono text-xs">
            {proxy.host}:{proxy.port}
          </span>
        ),
      },
      {
        key: "auth",
        header: "认证",
        width: "92px",
        hideOnMobile: true,
        render: (proxy) =>
          proxy.username ? (
            <span className="text-xs text-muted-foreground">{proxy.username}</span>
          ) : (
            <span className="text-xs text-muted-foreground">无</span>
          ),
      },
      {
        key: "fallback",
        header: "回退",
        width: "96px",
        hideOnMobile: true,
        render: (proxy) => (
          <StatusBadge
            tone={proxy.fallback === "direct" ? "warning" : "neutral"}
            label={proxy.fallback === "direct" ? "直连" : "失败"}
            dot={false}
          />
        ),
      },
      {
        key: "accounts",
        header: "绑定账号",
        width: "96px",
        render: (proxy) => (
          <span className="tabular-nums text-xs">{proxy.accountCount}</span>
        ),
      },
      {
        key: "status",
        header: "状态",
        width: "84px",
        render: (proxy) => (
          <StatusBadge
            tone={proxy.enabled ? "success" : "neutral"}
            label={proxy.enabled ? "启用" : "停用"}
            dot={false}
          />
        ),
      },
      {
        key: "actions",
        header: "操作",
        width: "96px",
        align: "center",
        render: (proxy) => (
          <RowActions
            actions={[
              {
                id: "edit",
                label: "编辑",
                icon: PencilLine,
                onClick: () => {
                  setEditing(proxy);
                  setFormOpen(true);
                },
              },
              {
                id: "delete",
                label: "删除",
                icon: Trash2,
                tone: "danger",
                onClick: () => setDeleteTarget(proxy),
              },
            ]}
          />
        ),
      },
    ],
    [],
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
            placeholder="搜索名称或地址"
            aria-label="搜索代理"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

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
            新建代理
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
        isLoading={isLoading && proxies.length === 0}
        text="正在加载代理..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={paged}
          rowKey={(proxy) => String(proxy.id)}
          empty={
            <EmptyState
              icon={Network}
              title="还没有代理"
              description="所有账号共用服务器出口 IP 时，很容易被上游判定成异常流量。给账号绑不同代理可以把风险摊开。"
              action={
                <Button
                  size="sm"
                  onClick={() => {
                    setEditing(null);
                    setFormOpen(true);
                  }}
                >
                  <Plus aria-hidden="true" />
                  新建代理
                </Button>
              }
            />
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

      <ProxyFormDialog
        open={formOpen}
        proxy={editing}
        onOpenChange={setFormOpen}
        onSaved={() => {
          setFormOpen(false);
          setReloadKey((value) => value + 1);
        }}
      />

      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(next) => !next && setDeleteTarget(null)}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>删除代理</DialogTitle>
            <DialogDescription>
              删除「{deleteTarget?.name}」后，
              {deleteTarget && deleteTarget.accountCount > 0
                ? `绑定的 ${deleteTarget.accountCount} 个账号会改为直连。`
                : "当前没有账号绑定它。"}
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
                void deleteProxy(target.id)
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

const emptyForm: ProxyPayload = {
  name: "",
  protocol: "socks5",
  host: "",
  port: 1080,
  username: "",
  password: "",
  fallback: "fail",
  enabled: true,
  note: "",
};

const ProxyFormDialog = ({
  open,
  proxy,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  proxy: Proxy | null;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) => {
  const [form, setForm] = useState<ProxyPayload>(emptyForm);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  const isEdit = proxy !== null;

  useEffect(() => {
    if (!open) {
      return;
    }
    setError("");
    setForm(
      proxy
        ? {
            name: proxy.name,
            protocol: proxy.protocol,
            host: proxy.host,
            port: proxy.port,
            username: proxy.username,
            // 编辑时不回填密码（后端也不返回），留空即不改。
            password: "",
            fallback: proxy.fallback,
            enabled: proxy.enabled,
            note: proxy.note,
          }
        : emptyForm,
    );
  }, [open, proxy]);

  const update = <K extends keyof ProxyPayload>(key: K, value: ProxyPayload[K]) =>
    setForm((prev) => ({ ...prev, [key]: value }));

  const submit = async () => {
    if (!form.name.trim() || !form.host.trim()) {
      setError("名称与地址都不能为空");
      return;
    }

    setPending(true);
    setError("");
    try {
      const payload: ProxyPayload = { ...form, name: form.name.trim(), host: form.host.trim() };
      // 编辑时密码留空 = 不改：传 undefined 而不是空串，
      // 否则后端会把它当成"把密码清空"。
      if (isEdit && !payload.password) {
        delete payload.password;
      }

      if (isEdit) {
        await updateProxy(proxy.id, payload);
      } else {
        await createProxy(payload);
      }
      onSaved();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{isEdit ? `编辑代理 · ${proxy?.name}` : "新建代理"}</DialogTitle>
          <DialogDescription>
            代理用于把不同账号的出口 IP 摊开，降低被上游判定成异常流量的概率。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">名称</label>
            <input
              value={form.name}
              onChange={(event) => update("name", event.target.value)}
              placeholder="例如：住宅-1"
              className={inputClass}
            />
          </div>

          <div className="grid grid-cols-3 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">协议</label>
              <Select
                value={form.protocol}
                onValueChange={(value) => update("protocol", value)}
              >
                <SelectTrigger aria-label="协议">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="socks5">SOCKS5</SelectItem>
                  <SelectItem value="http">HTTP</SelectItem>
                  <SelectItem value="https">HTTPS</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="col-span-2 flex flex-col gap-1">
              <label className="text-xs font-medium">地址</label>
              <input
                value={form.host}
                onChange={(event) => update("host", event.target.value)}
                placeholder="127.0.0.1"
                className={cn(inputClass, "font-mono text-xs")}
              />
            </div>
          </div>

          <div className="grid grid-cols-3 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">端口</label>
              <input
                value={String(form.port)}
                onChange={(event) => update("port", Number(event.target.value) || 0)}
                inputMode="numeric"
                className={cn(inputClass, "tabular-nums")}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">用户名（可选）</label>
              <input
                value={form.username}
                onChange={(event) => update("username", event.target.value)}
                className={cn(inputClass, "font-mono text-xs")}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">密码（可选）</label>
              <input
                type="password"
                value={form.password ?? ""}
                onChange={(event) => update("password", event.target.value)}
                placeholder={isEdit && proxy?.hasPassword ? "留空则不修改" : ""}
                className={cn(inputClass, "font-mono text-xs")}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">代理不可用时</label>
              <Select
                value={form.fallback}
                onValueChange={(value) => update("fallback", value as "fail" | "direct")}
              >
                <SelectTrigger aria-label="回退策略">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="fail">直接失败（不暴露真实 IP）</SelectItem>
                  <SelectItem value="direct">改走直连（可用性优先）</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">状态</label>
              <Select
                value={form.enabled ? "on" : "off"}
                onValueChange={(value) => update("enabled", value === "on")}
              >
                <SelectTrigger aria-label="状态">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="on">启用</SelectItem>
                  <SelectItem value="off">停用（等同于未绑定）</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <label className="text-xs font-medium">备注（可选）</label>
            <input
              value={form.note}
              onChange={(event) => update("note", event.target.value)}
              className={inputClass}
            />
          </div>

          {error ? <ErrorState title="保存失败" message={error} /> : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
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
