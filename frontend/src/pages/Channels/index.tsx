import { Boxes, Eye, PencilLine, Plus, RefreshCw, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { listChannels } from "@/api/channels";
import type { Channel } from "@/api/types";
import { DataTable, type Column } from "@/components/common/DataTable";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { Pagination } from "@/components/common/Pagination";
import { RowActions } from "@/components/common/RowActions";
import { StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import { listCatalogModels } from "@/api/catalog";
import { deleteChannel } from "@/api/channels";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ChannelFormSheet } from "@/pages/Channels/components/ChannelFormSheet";
import { platformLabel } from "@/pages/Channels/shared";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

export const ChannelsPage = () => {
  const [channels, setChannels] = useState<Channel[]>([]);
  // 模型目录：给分组的「对外模型」白名单做选项。
  const [models, setModels] = useState<Array<{ name: string }>>([]);
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [keyword, setKeyword] = useState("");
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  /** 正在确认删除的分组；null = 对话框关闭。 */
  const [deleting, setDeleting] = useState<Channel | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [reloadKey, setReloadKey] = useState(0);
  const navigate = useNavigate();

  const load = useCallback(async () => {
    setIsLoading(true);
    try {
      const [response, modelResponse] = await Promise.all([
        listChannels(),
        // 模型目录拉不到不影响分组列表：白名单那块会显示"还没有模型目录"。
        listCatalogModels().catch(() => null),
      ]);
      setChannels(response.data ?? []);
      setModels(modelResponse?.data?.models ?? []);
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

  /**
   * 删除分组。
   *
   * **会连带删掉组内账号**（外键是 CASCADE）：账号是「这个分组下的号」，
   * 分组没了留着它们只会变成没有归属的孤儿。所以确认框里必须写明这一点，
   * 并给出当前账号数 —— 让人知道自己在删什么。
   */
  const confirmDelete = useCallback(async () => {
    if (!deleting) {
      return;
    }
    setDeletePending(true);
    setDeleteError("");
    try {
      await deleteChannel(deleting.id);
      setDeleting(null);
      setReloadKey((value) => value + 1);
    } catch (requestError) {
      setDeleteError(getRequestErrorMessage(requestError));
    } finally {
      setDeletePending(false);
    }
  }, [deleting]);

  const filtered = useMemo(() => {
    const trimmed = keyword.trim().toLowerCase();
    if (!trimmed) {
      return channels;
    }

    return channels.filter(
      (channel) =>
        channel.name.toLowerCase().includes(trimmed) ||
        channel.id.toLowerCase().includes(trimmed) ||
        (channel.platforms ?? []).some((platform) =>
          platform.toLowerCase().includes(trimmed),
        ),
    );
  }, [channels, keyword]);

  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const paged = useMemo(
    () => filtered.slice((page - 1) * pageSize, page * pageSize),
    [filtered, page, pageSize],
  );

  // 搜索词变了就回到第一页：否则会停在一个"越界"的页码上，看起来像没数据。
  useEffect(() => {
    setPage(1);
  }, [keyword]);

  const openCreate = useCallback(() => {
    setEditing(null);
    setFormOpen(true);
  }, []);

  const openEdit = useCallback((channel: Channel) => {
    setEditing(channel);
    setFormOpen(true);
  }, []);

  const columns: Array<Column<Channel>> = [
    {
      key: "name",
      header: "名称",
      // 给一个**较小的基准宽度**：`table-fixed` 下浏览器会把剩余空间平均分给
      // 所有未设宽的列，只有一个未设宽时它就独吞剩余宽度（实测占 70%）。
      // 设成 small 之后，剩余空间按比例分给这一列，看起来才像正常表格。
      width: "26%",
      render: (channel) => (
        <Link
          to={`/channels/${encodeURIComponent(channel.id)}`}
          className="flex min-w-0 items-baseline gap-1.5 hover:text-primary"
        >
          <span className="truncate font-medium">{channel.name}</span>
          <span className="shrink-0 font-mono text-xs text-muted-foreground">
            {channel.id}
          </span>
        </Link>
      ),
    },
    {
      key: "platforms",
      header: "平台",
      width: "150px",
      // 渠道是计价分组，下面可能混不同平台的号 —— 显示的是"这个分组里
      // 有哪些平台"，而不是"这个渠道是什么类型"（后者已经不存在了）。
      render: (channel) => {
        const platforms = channel.platforms ?? [];
        if (platforms.length === 0) {
          return <span className="text-xs text-muted-foreground">—</span>;
        }
        return (
          <div className="flex flex-wrap items-center gap-1">
            {platforms.slice(0, 2).map((platform) => (
              <StatusBadge
                key={platform}
                tone="neutral"
                label={platformLabel(platform)}
                dot={false}
              />
            ))}
            {platforms.length > 2 ? (
              <span className="text-[11px] text-muted-foreground">
                +{platforms.length - 2}
              </span>
            ) : null}
          </div>
        );
      },
    },
    {
      key: "accounts",
      header: "账号",
      width: "72px",
      align: "center",
      render: (channel) => <span className="tabular-nums">{channel.accountCount}</span>,
    },
    {
      key: "requests",
      header: "今日调用",
      width: "96px",
      align: "center",
      hideOnMobile: true,
      render: (channel) => <span className="tabular-nums">{channel.todayRequests}</span>,
    },
    {
      key: "ratio",
      header: "倍率",
      width: "72px",
      align: "center",
      hideOnMobile: true,
      render: (channel) => (
        <span className="tabular-nums">×{channel.channelRatio.toFixed(2)}</span>
      ),
    },
    {
      key: "status",
      header: "状态",
      width: "96px",
      render: (channel) => (
        <StatusBadge
          tone={channel.enabled ? "success" : "neutral"}
          label={channel.enabled ? "已启用" : "已停用"}
        />
      ),
    },
    {
      key: "actions",
      header: "操作",
      width: "112px",
      align: "center",
      render: (channel) => (
        <RowActions
          actions={[
            {
              label: "编辑",
                id: "edit",
              icon: PencilLine,
              tone: "primary",
              onClick: () => openEdit(channel),
            },
            {
              label: "查看详情",
                id: "detail",
              icon: Eye,
              onClick: () => navigate(`/channels/${encodeURIComponent(channel.id)}`),
            },
            {
              label: "删除",
              id: "delete",
              icon: Trash2,
              tone: "danger",
              onClick: () => setDeleting(channel),
            },
          ]}
        />
      ),
    },
  ];

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
            placeholder="搜索名称、id 或类型"
            aria-label="搜索分组"
            className={cn(
              "h-8 w-full rounded-md border border-input bg-background pr-2.5 pl-8 text-sm",
              "placeholder:text-muted-foreground",
              "hover:border-border-strong",
              "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
            )}
          />
        </div>

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

        <Button size="sm" className="ml-auto" onClick={openCreate}>
          <Plus aria-hidden="true" />
          新建分组
        </Button>
      </div>

      {error ? (
        <ErrorState
          message={error}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      ) : null}

      <MyBox
        isLoading={isLoading && channels.length === 0}
        text="正在加载渠道..."
        className="flex min-h-0 flex-1 flex-col"
      >
        <DataTable
          columns={columns}
          rows={paged}
          rowKey={(channel) => channel.id}
          empty={
            channels.length === 0 ? (
              <EmptyState
                icon={Boxes}
                title="还没有分组"
                description="分组是计价层：决定倍率、优先级与调度权重。先建一个，再把账号放进来。"
                action={
                  <Button size="sm" onClick={openCreate}>
                    <Plus aria-hidden="true" />
                    新建分组
                  </Button>
                }
              />
            ) : (
              <EmptyState
                icon={Search}
                title="没有匹配的分组"
                description={`没有名称、id 或平台包含「${keyword}」的分组。`}
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

      <ChannelFormSheet
        open={formOpen}
        channel={editing}
        models={models}
        onOpenChange={setFormOpen}
        onSaved={() => {
          setFormOpen(false);
          setReloadKey((value) => value + 1);
        }}
      />

      {/* 删除确认：必须说清"会连带删掉账号"，这是不可撤销的操作。 */}
      <Dialog open={deleting !== null} onOpenChange={(next) => !next && setDeleting(null)}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>删除分组 · {deleting?.name}</DialogTitle>
            <DialogDescription>
              {deleting && deleting.accountCount > 0
                ? `该分组下有 ${deleting.accountCount} 个账号，删除分组会一并删掉它们。此操作不可撤销。`
                : "该分组下没有账号，删除后不可撤销。"}
            </DialogDescription>
          </DialogHeader>

          <p className="rounded-md bg-warning/10 px-3 py-2 text-[11px] leading-5 text-warning">
            账号是「分组下的号」，分组没了它们会变成没有归属的孤儿，所以一并删除。
            如果只是不想让它对外提供服务，请改为**停用**分组。
          </p>

          {deleteError ? <ErrorState title="删除失败" message={deleteError} /> : null}

          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setDeleting(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              onClick={() => void confirmDelete()}
              disabled={deletePending}
            >
              {deletePending ? "删除中..." : "确认删除"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};