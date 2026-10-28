import { ArrowLeft, Boxes, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { getChannel, listChannelModels, probeChannel } from "@/api/channels";
import type { ChannelModelEntry } from "@/api/channels";
import type { Channel } from "@/api/types";
import { EmptyState } from "@/components/common/EmptyState";
import { ErrorState } from "@/components/common/ErrorState";
import { MyBox } from "@/components/common/MyBox";
import { PageSection, StatusBadge } from "@/components/common/StatusBadge";
import { Button } from "@/components/ui/button";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { ChannelAccountsTab } from "@/pages/Channels/components/ChannelAccountsTab";
import { ChannelUsageTab } from "@/pages/Channels/components/ChannelUsageTab";
import { adapterLabel } from "@/pages/Channels/shared";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

export const ChannelDetailPage = () => {
  const { id = "" } = useParams<{ id: string }>();
  const [channel, setChannel] = useState<Channel | null>(null);
  const [models, setModels] = useState<ChannelModelEntry[]>([]);
  const [error, setError] = useState("");
  const [modelsError, setModelsError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [modelsLoading, setModelsLoading] = useState(true);
  const [probing, setProbing] = useState(false);
  const [probeMessage, setProbeMessage] = useState("");
  const [reloadKey, setReloadKey] = useState(0);

  const load = useCallback(async () => {
    setIsLoading(true);
    setModelsLoading(true);

    try {
      const response = await getChannel(id);
      setChannel(response.data ?? null);
      setError("");
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setIsLoading(false);
    }

    try {
      const response = await listChannelModels(id);
      setModels(response.data ?? []);
      setModelsError("");
    } catch (requestError) {
      setModels([]);
      setModelsError(getRequestErrorMessage(requestError));
    } finally {
      setModelsLoading(false);
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load, reloadKey]);

  const handleProbe = useCallback(async () => {
    setProbing(true);
    setProbeMessage("");
    try {
      const response = await probeChannel(id);
      const result = response.data;
      setProbeMessage(
        result?.ok
          ? `探活成功 · ${result.model} · ${result.latencyMs}ms`
          : `探活失败：${result?.error ?? "上游未返回原因"}`,
      );
    } catch (requestError) {
      setProbeMessage(`探活失败：${getRequestErrorMessage(requestError)}`);
    } finally {
      setProbing(false);
    }
  }, [id]);

  if (isLoading && !channel) {
    return (
      <MyBox isLoading text="正在加载渠道..." className="min-h-40">
        <div />
      </MyBox>
    );
  }

  if (error || !channel) {
    return (
      <div className="flex flex-col gap-3">
        <Button variant="ghost" size="sm" asChild className="self-start">
          <Link to="/channels">
            <ArrowLeft aria-hidden="true" />
            返回渠道列表
          </Link>
        </Button>
        <ErrorState
          message={error || "渠道不存在"}
          onRetry={() => setReloadKey((value) => value + 1)}
          retrying={isLoading}
        />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="ghost" size="sm" asChild>
          <Link to="/channels">
            <ArrowLeft aria-hidden="true" />
            返回
          </Link>
        </Button>
        <span className="font-mono text-xs text-muted-foreground">{channel.id}</span>
        <StatusBadge
          tone={channel.enabled ? "success" : "neutral"}
          label={channel.enabled ? "已启用" : "已停用"}
        />
        <span className="text-xs text-muted-foreground">
          {adapterLabel(channel.adapter)} · 倍率 ×{channel.channelRatio.toFixed(2)}
        </span>

        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void handleProbe()}
            disabled={probing}
          >
            <RefreshCw
              className={cn(probing && "animate-spin motion-reduce:animate-none")}
              aria-hidden="true"
            />
            {probing ? "探活中..." : "探活"}
          </Button>
        </div>
      </div>

      {probeMessage ? (
        <p className="rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          {probeMessage}
        </p>
      ) : null}

      <Tabs defaultValue="accounts" className="min-h-0">
        <TabsList>
          <TabsTrigger value="accounts">账号</TabsTrigger>
          <TabsTrigger value="models">模型</TabsTrigger>
          <TabsTrigger value="usage">用量</TabsTrigger>
        </TabsList>

        <TabsContent value="accounts" className="min-h-0">
          <ChannelAccountsTab channel={channel} />
        </TabsContent>

        <TabsContent value="models" className="min-h-0">
          <PageSection
            title="渠道模型目录"
            description="实时拉取的上游模型；未在「模型」表定价的条目会高亮提示"
            bodyClassName="p-0"
          >
            {modelsLoading ? (
              <MyBox isLoading text="正在拉取目录...">
                <div className="h-24" />
              </MyBox>
            ) : modelsError ? (
              <div className="p-4">
                <ErrorState
                  message={modelsError}
                  onRetry={() => setReloadKey((value) => value + 1)}
                />
              </div>
            ) : models.length === 0 ? (
              <EmptyState
                icon={Boxes}
                title="该渠道没有返回模型"
                description="上游目录为空，或适配器尚未实现 list_models。"
              />
            ) : (
              <ul className="divide-y">
                {models.map((model) => (
                  <li
                    key={model.id}
                    className="flex flex-wrap items-center gap-2 px-4 py-2 text-sm"
                  >
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">
                      {model.id}
                    </span>
                    {model.upstreamId !== model.id ? (
                      <span className="text-[11px] text-muted-foreground">
                        → {model.upstreamId}
                      </span>
                    ) : null}
                    {model.contextWindow ? (
                      <span className="text-[11px] text-muted-foreground tabular-nums">
                        {(model.contextWindow / 1000).toFixed(0)}K
                      </span>
                    ) : null}
                    <StatusBadge
                      tone={model.source === "live" ? "info" : "neutral"}
                      label={model.source === "live" ? "实时" : "快照"}
                    />
                  </li>
                ))}
              </ul>
            )}
          </PageSection>
        </TabsContent>

        <TabsContent value="usage" className="min-h-0">
          <ChannelUsageTab channelId={channel.id} />
        </TabsContent>
      </Tabs>
    </div>
  );
};
