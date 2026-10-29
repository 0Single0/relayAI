import { useState } from "react";
import { cn } from "@/lib/utils";
import { logoUrl, vendorOf } from "@/components/common/ModelLogo/vendor";

/**
 * 模型厂商 logo。
 *
 * 优先用**官方 logo**（Simple Icons 的官方 SVG，见 `vendor.ts`），
 * 取不到时回落到字母标记 —— 图标是装饰，不该因为它加载失败
 * 就让整个列表出现空洞。
 *
 * 用 `<img>` 而不是内联 SVG：logo 来自外部 CDN，
 * 内联就要先下载再塞进 DOM，多一层状态管理；`<img>` 失败时正好触发回落。
 */
export const ModelLogo = ({
  model,
  size = "md",
  className,
}: {
  model: string;
  size?: "sm" | "md";
  className?: string;
}) => {
  const vendor = vendorOf(model);
  const url = logoUrl(vendor);
  const [failed, setFailed] = useState(false);

  const box = size === "sm" ? "size-5" : "size-8";
  const showImage = url !== null && !failed;

  return (
    <span
      title={vendor.label}
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-md",
        box,
        /*
          有 logo 时**不加边框也不加底色**：
          logo 本身就是该厂商的品牌色，套一圈 ring 会把它变成"带框的图标"，
          看起来像张没加载完的占位图。留白 + 图形本身就够识别。
          无 logo 时才用彩色底衬托首字母标记。
        */
        showImage ? "" : vendor.className,
        className,
      )}
    >
      {showImage ? (
        <img
          src={url}
          alt={vendor.label}
          loading="lazy"
          decoding="async"
          onError={() => setFailed(true)}
          className={cn(
            "object-contain",
            size === "sm" ? "size-3" : "size-5",
          )}
        />
      ) : (
        <span
          aria-label={vendor.label}
          className={cn(
            "font-semibold",
            size === "sm" ? "text-[9px]" : "text-[11px]",
          )}
        >
          {vendor.mark}
        </span>
      )}
    </span>
  );
};
