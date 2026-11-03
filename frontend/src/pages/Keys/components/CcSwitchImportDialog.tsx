import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { importToCcSwitch, type CcSwitchClientType } from "@/lib/ccswitch";
import { toast } from "@/components/common/Toast/store";

/**
 * 「导入到 CC Switch」对话框。
 *
 * 只做一件事：让用户选一个客户端，再把 `ccswitch://` 链接交给 CC Switch。
 * 三个按钮用**各产品真实的官方 logo**（`public/logos/`，本地文件，
 * 不依赖外网 CDN —— 国内访问 cdn.simpleicons.org 经常 404/超时）：
 *   · Claude Code → Claude 的橙色星芒；
 *   · Codex CLI   → OpenAI 的官方徽标；
 *   · Gemini CLI  → Google Gemini 的四角星。
 *
 * 按钮上**不显示 `app=xxx`**：那是 deeplink 的查询参数，属于实现细节，
 * 摆给用户看只会让人以为要自己拼 URL。
 */

type CcSwitchClient = {
  type: CcSwitchClientType;
  label: string;
  icon: string;
};

const CC_SWITCH_CLIENTS: CcSwitchClient[] = [
  { type: "claude", label: "Claude Code", icon: "/logos/claude.svg" },
  { type: "codex", label: "Codex CLI", icon: "/logos/openai.svg" },
  { type: "gemini", label: "Gemini CLI", icon: "/logos/googlegemini.svg" },
];

export const CcSwitchImportDialog = ({
  open,
  gatewayBaseUrl,
  apiKey,
  onOpenChange,
}: {
  open: boolean;
  /** 后端下发的网关地址；为空时禁用导入（拼不出合法链接）。 */
  gatewayBaseUrl: string;
  /** 要写进 provider 的密钥明文。 */
  apiKey: string;
  onOpenChange: (next: boolean) => void;
}) => {
  const handlePick = (clientType: CcSwitchClientType) => {
    onOpenChange(false);
    const ok = importToCcSwitch({
      baseUrl: gatewayBaseUrl,
      providerName: "relayAI",
      apiKey,
      clientType,
    });
    if (ok) {
      toast.success("已发送到 CC Switch", "请在客户端里确认新增的 provider");
      return;
    }
    toast.info(
      "已尝试唤起 CC Switch",
      "若没有反应，说明本机没有安装或未注册 ccswitch:// 协议",
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
              客户端使用此地址接入，导入配置也会使用相同地址。
            </p>
          </div>

          <div className="grid grid-cols-3 gap-2">
            {CC_SWITCH_CLIENTS.map((client) => (
              <Button
                key={client.type}
                variant="outline"
                size="sm"
                disabled={!gatewayBaseUrl}
                onClick={() => handlePick(client.type)}
                className="flex h-auto flex-col items-center gap-2 py-3.5"
              >
                <img
                  src={client.icon}
                  alt=""
                  aria-hidden="true"
                  className="size-7 object-contain"
                />
                <span className="text-xs font-medium">{client.label}</span>
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
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            取消
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
