import { useCallback, useEffect, useMemo, useState } from "react";
import { ExternalLink, KeyRound, Loader2 } from "lucide-react";
import {
  completeLogin,
  createCredentialAccount,
  pollLogin,
  startLogin,
  type LoginTicket,
} from "@/api/accounts";
import { createChannel, listChannels } from "@/api/channels";
import type { Channel } from "@/api/types";
import { toast } from "@/components/common/Toast/store";
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
import {
  outboundOptions,
  platformOptions,
  realmOptions,
  upstreamProtocolOptions,
} from "@/pages/Channels/shared";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

type CreateAccountDrawerProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
  /** 预设渠道（从渠道详情页进来时用）。 */
  defaultChannelId?: string;
};

/**
 * 「新建账号」。
 *
 * **顺序是"先选平台，再选计价分组"** —— 这是用户脑子里的顺序：
 * "我要加一个 WorkBuddy 号，算到低价通道里"。
 *
 * 平台在账号上（`accounts.platform`），所以：
 *   · 不需要先建一个「WorkBuddy 类型」的渠道；
 *   · 同一个计价分组下面可以混不同平台的号；
 *   · realm / 出站身份 / Base URL / 协议 都跟着这个号走。
 *
 * 两种凭据形态：
 *   1. OAuth 平台（WorkBuddy / Trae）：发起授权，成功后自动建号；
 *   2. 自定义供应商：填 Base URL + 协议 + API Key。
 *
 * 渠道怎么来：选中计价分组后，若还没有任何渠道就顺手建一个默认的 ——
 * 用户只想加个号，不该被迫先想"渠道叫什么"。
 */
export const CreateAccountDrawer = ({
  open,
  onOpenChange,
  onCreated,
  defaultChannelId,
}: CreateAccountDrawerProps) => {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [platform, setPlatform] = useState("workbuddy");
  const [channelId, setChannelId] = useState(defaultChannelId ?? "");
  const [nickname, setNickname] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [protocol, setProtocol] = useState("openai-chat");
  const [realm, setRealm] = useState("cn");
  const [outbound, setOutbound] = useState("desktop");
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [ticket, setTicket] = useState<LoginTicket | null>(null);
  const [callback, setCallback] = useState("");

  const loadChannels = useCallback(async () => {
    try {
      const response = await listChannels();
      setChannels(response.data ?? []);
    } catch {
      // 拉不到渠道不影响建号（会走"顺手建一个"的分支），不打断用户。
    }
  }, []);

  useEffect(() => {
    if (open) {
      void loadChannels();
    }
  }, [loadChannels, open]);

  useEffect(() => {
    if (!open) {
      setNickname("");
      setApiKey("");
      setBaseUrl("");
      setMessage("");
      setError("");
      setTicket(null);
      setCallback("");
    }
  }, [open]);

  useEffect(() => {
    if (defaultChannelId && channels.some((c) => c.id === defaultChannelId)) {
      return;
    }
    // 默认落到第一个渠道：多数人只有一个计价分组。
    setChannelId((current) => current || channels[0]?.id || "");
  }, [channels, defaultChannelId]);

  const selectedPlatform = useMemo(
    () => platformOptions.find((item) => item.value === platform),
    [platform],
  );
  const needsOAuth = selectedPlatform?.oauth ?? false;
  const isWorkbuddy = platform === "workbuddy";

  /** 确保有一个可用渠道：有就复用，没有就建一个默认的。 */
  const ensureChannel = useCallback(async (): Promise<string> => {
    if (channelId) {
      return channelId;
    }

    const response = await createChannel({
      id: `default-${Date.now().toString(36)}`,
      name: "默认通道",
      adapter: "custom",
      enabled: true,
      priority: 100,
      weight: 1,
      options: {},
      channelRatio: 1,
      concurrencyLimit: null,
      rateLimitQps: null,
    });

    const created = response.data;
    if (!created) {
      throw new Error("渠道创建失败：后端没有返回渠道");
    }
    setChannels((prev) => [...prev, created]);
    setChannelId(created.id);
    return created.id;
  }, [channelId]);

  const submitApiKey = useCallback(async () => {
    if (!apiKey.trim()) {
      setError("API Key 不能为空");
      return;
    }
    if (!baseUrl.trim()) {
      setError("自定义供应商必须填上游地址");
      return;
    }
    if (!/^https?:\/\//i.test(baseUrl.trim())) {
      setError("上游地址必须以 http:// 或 https:// 开头");
      return;
    }

    setPending(true);
    setError("");
    try {
      const target = await ensureChannel();
      await createCredentialAccount({
        channelId: target,
        // uid 用 Key 前缀 + 时间戳：自定义供应商没有上游账号概念，
        // 需要一个内部唯一标识来区分多条 Key。
        uid: `${apiKey.trim().slice(0, 6)}-${Date.now().toString(36)}`,
        nickname: nickname.trim() || "自定义 Key",
        apiKey: apiKey.trim(),
        platform: "custom",
        baseUrl: baseUrl.trim(),
        protocol,
      });
      toast.success("账号已添加");
      onCreated();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  }, [apiKey, baseUrl, ensureChannel, nickname, onCreated, protocol]);

  const beginLogin = useCallback(async () => {
    setPending(true);
    setError("");
    setTicket(null);
    try {
      const target = await ensureChannel();
      const response = await startLogin(target, platform, isWorkbuddy ? realm : undefined);
      const result = response.data;
      if (!result) {
        setError("后端没有返回登录票据");
        return;
      }
      setTicket(result);
      setMessage(result.hint);

      if (result.hasAuthPage && !result.needsCallback) {
        window.open(result.authUrl, "_blank", "noopener,noreferrer");
      }
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  }, [ensureChannel, isWorkbuddy, platform, realm]);

  /**
   * 自动轮询授权状态。
   *
   * 为什么必须自动：用户在浏览器里完成上游授权后**不会回到这个弹窗**操作 ——
   * 他刚在另一个标签页点了"允许"，回来只期待看到"好了"。
   * 让他自己点「检查授权状态」，等于把系统该做的事推给人。
   *
   * 间隔 2 秒：上游回调到网关落库通常在 1–3 秒内完成；
   * 更快没意义（大部分次都是空的），更慢会让用户觉得卡住。
   *
   * 只在 OAuth 流程（有 ticket 且需要回调）里跑，`needsCallback` 的平台
   * 要用户贴回调地址，那种没法自动。
   */
  useEffect(() => {
    if (!open || !ticket || !channelId || ticket.needsCallback) {
      return;
    }

    let cancelled = false;
    let timer = 0;

    const tick = async () => {
      try {
        const response = await pollLogin(channelId, ticket.state, platform);
        if (cancelled) {
          return;
        }
        const result = response.data;
        if (result?.state === "ready") {
          // 成功：直接关弹窗并让列表刷新 —— 用户不需要再点任何东西。
          toast.success(result.message || "账号已添加");
          onCreated();
          onOpenChange(false);
          return;
        }
        if (result?.state === "failed") {
          setError(result.message || "授权失败");
          return;
        }
      } catch {
        // 轮询失败不弹错误：网络抖动很常见，下一轮会自己好。
        // 真正的失败（授权过期/被拒）由上面的 `failed` 分支处理。
      }
      if (!cancelled) {
        timer = window.setTimeout(() => void tick(), 2000);
      }
    };

    timer = window.setTimeout(() => void tick(), 2000);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [channelId, onCreated, onOpenChange, open, platform, ticket]);

  const submitCallback = useCallback(async () => {
    if (!ticket || !channelId || !callback.trim()) {
      return;
    }
    setPending(true);
    setError("");
    try {
      const response = await completeLogin(
        channelId,
        ticket.state,
        callback.trim(),
        platform,
      );
      const result = response.data;
      if (result?.state === "ready") {
        // 与自动轮询保持一致：成功后直接关窗，用户不需要再点"关闭"。
        toast.success(result.message || "账号已添加");
        onCreated();
        onOpenChange(false);
      } else {
        setError(result?.message ?? "回调地址无效");
      }
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  }, [callback, channelId, onCreated, onOpenChange, platform, ticket]);

  const inputClass = cn(
    "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm",
    "placeholder:text-muted-foreground",
    "hover:border-border-strong",
    "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>新建账号</DialogTitle>
          <DialogDescription>
            账号是一份上游凭据。选平台 → 选计价分组，凭据跟着这个号走。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          {/* 平台 */}
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium">平台</span>
            <div className="flex flex-wrap gap-1.5">
              {platformOptions.map((item) => (
                <button
                  key={item.value}
                  type="button"
                  onClick={() => setPlatform(item.value)}
                  aria-pressed={platform === item.value}
                  title={item.hint}
                  className={cn(
                    "rounded-lg border px-3 py-1.5 text-xs transition-colors",
                    "focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none",
                    platform === item.value
                      ? "border-primary bg-primary/8 font-medium text-primary"
                      : "hover:border-border-strong hover:bg-muted/50",
                  )}
                >
                  {item.label}
                </button>
              ))}
            </div>
            {selectedPlatform ? (
              <p className="text-[11px] text-muted-foreground">{selectedPlatform.hint}</p>
            ) : null}
          </div>

          {/* 计价分组 */}
          <div className="flex flex-col gap-1">
            <span className="text-xs font-medium">计价分组</span>
            {channels.length > 0 ? (
              <Select value={channelId} onValueChange={setChannelId}>
                <SelectTrigger aria-label="选择计价分组">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {channels.map((channel) => (
                    <SelectItem key={channel.id} value={channel.id}>
                      {channel.name}（×{channel.channelRatio.toFixed(2)}）
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <p className="text-[11px] text-muted-foreground">
                还没有计价分组，提交时会自动建一个「默认通道」。
              </p>
            )}
          </div>

          {/*
            显示名称只在**自定义供应商**时出现。

            OAuth 平台（WorkBuddy / Trae）在授权完成后，网关会拿着 token
            去问上游"这个号是谁"，把返回的昵称或邮箱写进 `accounts.nickname`。
            那个值才是真的，所以这里**连字段都不显示** ——
            显示一个"不需要你填"的说明，只是多一行噪音。
          */}
          {needsOAuth ? null : (
            <div className="flex flex-col gap-1">
              <span className="text-xs font-medium">显示名称</span>
              <input
                value={nickname}
                onChange={(event) => setNickname(event.target.value)}
                placeholder="例如：主力 Key"
                className={inputClass}
              />
              <p className="text-[11px] text-muted-foreground">
                自定义供应商拿不到上游身份，需要自己起个名字。
              </p>
            </div>
          )}

          {isWorkbuddy ? (
            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">区域</span>
                <Select value={realm} onValueChange={setRealm}>
                  <SelectTrigger aria-label="区域">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {realmOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">出站身份</span>
                <Select value={outbound} onValueChange={setOutbound}>
                  <SelectTrigger aria-label="出站身份">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {outboundOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
          ) : null}

          {needsOAuth ? (
            <div className="flex flex-col gap-2 rounded-lg border bg-muted/30 p-3">
              <p className="text-[11px] leading-5 text-muted-foreground">
                {message || "点「开始授权」会打开上游授权页；完成授权后这里会自动检测并关窗。"}
              </p>
              {ticket?.authUrl ? (
                <a
                  href={ticket.authUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-1 text-xs text-primary hover:underline"
                >
                  <ExternalLink className="size-3" aria-hidden="true" />
                  打开授权页
                </a>
              ) : null}
              {ticket?.needsCallback ? (
                <>
                  <textarea
                    value={callback}
                    onChange={(event) => setCallback(event.target.value)}
                    rows={2}
                    placeholder="授权后浏览器会跳到打不开的地址，把整条地址粘回来"
                    className={cn(
                      "w-full rounded-md border border-input bg-background px-2.5 py-2 font-mono text-xs",
                      "placeholder:text-muted-foreground",
                      "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
                    )}
                  />
                  <Button
                    size="sm"
                    className="self-start"
                    onClick={() => void submitCallback()}
                    disabled={pending || !callback.trim()}
                  >
                    提交回调
                  </Button>
                </>
              ) : null}
            </div>
          ) : (
            <>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">上游地址</span>
                <input
                  value={baseUrl}
                  onChange={(event) => setBaseUrl(event.target.value)}
                  placeholder="https://api.example.com/v1"
                  className={cn(inputClass, "font-mono text-xs")}
                />
                <p className="text-[11px] text-muted-foreground">
                  这把 Key 属于哪家上游。地址跟着 Key 走，所以填在这里而不是渠道上。
                </p>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">上游协议</span>
                <Select value={protocol} onValueChange={setProtocol}>
                  <SelectTrigger aria-label="上游协议">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {upstreamProtocolOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-medium">API Key</span>
                <input
                  value={apiKey}
                  onChange={(event) => setApiKey(event.target.value)}
                  type="password"
                  placeholder="sk-..."
                  className={cn(inputClass, "font-mono")}
                />
                <p className="text-[11px] text-muted-foreground">
                  加密后存储，明文不会返回给前端。
                </p>
              </div>
            </>
          )}

          {error ? <ErrorState title="添加失败" message={error} /> : null}

          {/*
            发起授权后显示"正在等待"，让人知道系统在自动盯着 ——
            没有这条提示，界面上什么都没变，用户会以为点了没反应。
          */}
          {!error && needsOAuth && ticket && !ticket.needsCallback ? (
            <p className="flex items-center gap-2 rounded-md border border-info/30 bg-info/10 px-3 py-2 text-xs text-info">
              <Loader2
                className="size-3.5 animate-spin motion-reduce:animate-none"
                aria-hidden="true"
              />
              在上游页面完成授权即可，这里会自动检测并关窗，不需要手动确认。
            </p>
          ) : null}

          {!error && message && !needsOAuth ? (
            <p className="rounded-md border border-success/30 bg-success/10 px-3 py-2 text-xs text-success">
              {message}
            </p>
          ) : null}
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
          {needsOAuth ? (
            <>
              {/*
                不再有「检查授权状态」按钮。
                授权完成后由上面的定时器自动发现、自动关窗、自动刷新列表 ——
                让用户自己点一下去"确认系统有没有收到"，是把系统该做的事推给人。
              */}
              <Button size="sm" onClick={() => void beginLogin()} disabled={pending}>
                {pending ? (
                  <Loader2 className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
                ) : null}
                {ticket ? "重新发起授权" : "开始授权"}
              </Button>
            </>
          ) : (
            <Button
              size="sm"
              onClick={() => void submitApiKey()}
              disabled={pending || !apiKey.trim()}
            >
              {pending ? (
                <Loader2 className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
              ) : (
                <KeyRound aria-hidden="true" />
              )}
              添加账号
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
