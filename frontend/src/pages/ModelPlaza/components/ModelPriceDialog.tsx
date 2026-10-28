import { useEffect, useState } from "react";
import { updateModelPrice, type CatalogModel } from "@/api/catalog";
import { toast } from "@/components/common/Toast/store";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

const inputClass = cn(
  "h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm tabular-nums",
  "placeholder:text-muted-foreground",
  "hover:border-border-strong",
  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 focus-visible:outline-none",
);

/**
 * 编辑单个模型的价格。
 *
 * 单位是**元 / 百万 token** —— 与页面展示一致。
 * 库里存的就是这个单位，所以不需要换算，也不会出现"页面写美元、库里存人民币"
 * 这种对不上的情况。
 *
 * 三个价分开填：
 *   · 输入价：未命中缓存的那部分输入；
 *   · 缓存读价：命中缓存的部分（通常只有输入价的 1/10）；
 *   · 输出价：生成的部分。
 *
 * 缓存价留空时按输入价计 —— 这是保守做法（宁可算高，不要漏账）。
 */
export const ModelPriceDialog = ({
  model,
  onClose,
  onSaved,
}: {
  model: CatalogModel | null;
  onClose: () => void;
  onSaved: () => void;
}) => {
  const [inputPrice, setInputPrice] = useState("");
  const [outputPrice, setOutputPrice] = useState("");
  const [cacheReadPrice, setCacheReadPrice] = useState("");
  const [modelRatio, setModelRatio] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [pending, setPending] = useState(false);


  useEffect(() => {
    if (model) {
      // 显示两位小数：价格量级从 0.14 到 50，再多的位数意义不大。
      setInputPrice(model.inputPrice > 0 ? model.inputPrice.toFixed(2) : "");
      setOutputPrice(model.outputPrice > 0 ? model.outputPrice.toFixed(2) : "");
      setCacheReadPrice(model.cacheReadPrice > 0 ? model.cacheReadPrice.toFixed(2) : "");
      setModelRatio(model.modelRatio > 0 ? String(model.modelRatio) : "1");
      setEnabled(model.enabled);

    }
  }, [model]);

  const submit = async () => {
    if (!model) {
      return;
    }

    const input = Number(inputPrice || 0);
    const output = Number(outputPrice || 0);
    const cache = Number(cacheReadPrice || 0);
    const ratio = Number(modelRatio || 1);

    if (!Number.isFinite(input) || !Number.isFinite(output) || !Number.isFinite(cache)) {
      toast.error("保存失败", "价格必须是数字");
      return;
    }
    if (input < 0 || output < 0 || cache < 0) {
      toast.error("保存失败", "价格不能为负数");
      return;
    }
    if (!Number.isFinite(ratio) || ratio <= 0) {
      toast.error("保存失败", "模型倍率必须大于 0");
      return;
    }

    setPending(true);
    try {
      await updateModelPrice(model.name, {
        inputPrice: input,
        outputPrice: output,
        cacheReadPrice: cache,
        modelRatio: ratio,
        enabled,
      });
      onSaved();
    } catch (requestError) {
      // 失败用 toast：对话框底部那行容易被忽略，
      // 而且弹窗本身可能因为滚动被遮住 —— 用户会以为"点了没反应"。
      toast.error("保存失败", getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={model !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>编辑价格 · {model?.displayName || model?.name}</DialogTitle>
          <DialogDescription>
            单位：元 / 百万 token。按官方牌价填；缓存读取价通常只有输入价的 1/10。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">输入价</label>
              <input
                value={inputPrice}
                onChange={(event) => setInputPrice(event.target.value)}
                placeholder="0.00"
                inputMode="decimal"
                className={inputClass}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">输出价</label>
              <input
                value={outputPrice}
                onChange={(event) => setOutputPrice(event.target.value)}
                placeholder="0.00"
                inputMode="decimal"
                className={inputClass}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">缓存读取价</label>
              <input
                value={cacheReadPrice}
                onChange={(event) => setCacheReadPrice(event.target.value)}
                placeholder="留空按输入价计"
                inputMode="decimal"
                className={inputClass}
              />
            </div>
            <div className="flex flex-col gap-1">
              <label className="text-xs font-medium">模型倍率</label>
              <input
                value={modelRatio}
                onChange={(event) => setModelRatio(event.target.value)}
                placeholder="1"
                inputMode="decimal"
                className={inputClass}
              />
            </div>
          </div>

          <div className="flex items-center justify-between rounded-md border px-3 py-2">
            <div className="flex flex-col">
              <span className="text-xs font-medium">对外启用</span>
              <span className="text-[11px] text-muted-foreground">
                停用后不出现在模型广场，也不参与调度
              </span>
            </div>
            <Switch
              checked={enabled}
              onCheckedChange={setEnabled}
              aria-label="启用模型"
            />
          </div>

          <p className="rounded-md bg-muted/50 px-3 py-2 text-[11px] leading-5 text-muted-foreground">
            记账额度 = (输入 × 输入价 + 缓存 × 缓存价 + 输出 × 输出价) ÷ 100 万
            × 模型倍率 × 分组倍率。改完价格会自动推给网关，下一条请求即按新价计费。
          </p>


        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={onClose}>
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
