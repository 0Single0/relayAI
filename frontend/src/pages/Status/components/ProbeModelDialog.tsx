import { useEffect, useMemo, useState } from "react";
import { updateChannel } from "@/api/channels";
import { listCatalogModels, type CatalogModel } from "@/api/catalog";
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
import { cn } from "@/lib/utils";
import { getRequestErrorMessage } from "@/utils/request";

/**
 * 「还没选」的哨兵值。
 *
 * Radix Select 不接受空字符串作为 value，所以用一个不可能的模型名占位。
 * 保存时空值转成空串 —— 后端据此表示"未指定"，探活时由网关兜底挑。
 */
const AUTO = "__auto__";

/**
 * 设置某个分组**用哪个模型探活**。
 *
 * 为什么需要这个设置：
 *   探活默认挑"轻量档"模型（名字带 flash / mini / nano 的那类）—— 探针会真实消耗额度。
 *   但这有个盲区：**轻量模型通了不代表真实模型能通**。
 *   分组只挂了推理模型、或某个号被单独限权时，探针会一直绿而真实请求照样失败。
 *
 * 所以允许指定一个"贴近真实用法"的模型。留空/选自动则回到轻量优先。
 */
export const ProbeModelDialog = ({
  channel,
  onClose,
  onSaved,
}: {
  /** 要编辑的分组；null 表示对话框关闭。 */
  channel: { id: string; name: string; probeModel: string } | null;
  onClose: () => void;
  onSaved: () => void;
}) => {
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [value, setValue] = useState(AUTO);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!channel) {
      return;
    }
    setValue(channel.probeModel || AUTO);
    setError("");
  }, [channel]);


  useEffect(() => {
    if (!channel) {
      return;
    }
    // 拉模型目录：失败不阻断对话框，退化成"只能选自动"。
    void listCatalogModels()
      .then((response) => setModels(response.data?.models ?? []))
      .catch(() => setModels([]));
  }, [channel]);

  /**
   * 只列**这个分组真的提供**的模型。
   *
   * 列全量会让人选到一个该分组根本没有的模型，探活必然失败 ——
   * 那种失败是配置错误，不是账号故障，会误导排查方向。
   */
  const options = useMemo(() => {
    if (!channel) {
      return [];
    }
    const list = models.filter((model) =>
      model.channels.some((item) => item.id === channel.id && item.exposed),
    );
    /*
      按"输入价 + 输出价"从低到高排。

      探活会真实消耗额度，用户来这里多半是想挑个便宜的号来省额度 ——
      把最便宜的放最前面，比按字母序更符合这个场景。
      未定价的排最后：它们的成本算不出来，不该被当成"免费"优先选。
    */
    const cost = (model: (typeof list)[number]) =>
      model.priced ? model.inputPrice + model.outputPrice : Number.POSITIVE_INFINITY;
    return [...list].sort((left, right) => cost(left) - cost(right));
  }, [channel, models]);
  /*
    已保存的探活模型是空（从未指定）时，选中列表里的第一个。

    为什么不再保留空的"未指定"态：下拉里已经没有「自动」这一项了，
    空值会让触发器显示成空白框 —— 看起来像坏了。
    选第一个是明确的可视默认值，用户能看见、也能改。
  */
  useEffect(() => {
    if (!channel || value !== AUTO) {
      return;
    }
    if (options.length > 0) {
      setValue(options[0].name);
    }
  }, [channel, options, value]);

  const submit = async () => {
    if (!channel) {
      return;
    }
    setPending(true);
    setError("");
    try {
      await updateChannel(channel.id, { probeModel: value === AUTO ? "" : value });
      onSaved();
    } catch (requestError) {
      setError(getRequestErrorMessage(requestError));
    } finally {
      setPending(false);
    }
  };

  return (
    <Dialog open={channel !== null} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>探活模型 · {channel?.name}</DialogTitle>
          <DialogDescription>
            探活会用这个模型发一句 hello 来确认凭据可用。请选一个该分组真实提供的模型。
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-medium">使用模型</span>
            <Select value={value} onValueChange={setValue}>
              <SelectTrigger aria-label="探活模型">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {/*
                  只列模型名，**不显示价格、也不提供「自动」**。

                  不做自动挑选的原因：自动挑出来的模型用户看不见依据，
                  出问题时无从判断"为什么用了这个"。让人自己选，
                  选择本身就是可解释的。

                  价格不显示：这里的任务是"确认凭据能用"，不是比价；
                  而且未定价的模型显示价格只会变成 0，反而不准。
                */}
                {options.map((model) => (
                  <SelectItem key={model.name} value={model.name}>
                    {model.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              只列出这个分组对外提供的模型。自动模式下网关优先挑免费档；
              如果免费模型不能代表真实能力（比如分组只挂了推理模型），在这里指定。
            </p>
          </div>

          {options.length === 0 ? (
            <p
              className={cn(
                "rounded-md bg-muted/50 px-3 py-2 text-xs leading-5 text-muted-foreground",
              )}
            >
              这个分组还没有对外提供的模型，暂时只能用自动模式。
              先给它添加账号，网关拉到模型目录后再回来指定。
            </p>
          ) : null}

          {error ? <ErrorState title="保存失败" message={error} /> : null}
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
