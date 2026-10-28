import { Eye, EyeOff, type LucideIcon } from "lucide-react";
import { useId, useState, type ComponentProps } from "react";
import { cn } from "@/lib/utils";

export type AuthFieldProps = {
  label: string;
  icon: LucideIcon;
  /** 注册时密码要给出长度提示；登录时不显示。 */
  hint?: string;
  /** 提交后才有值，输入时清空。 */
  error?: string;
  optional?: boolean;
} & Omit<ComponentProps<"input">, "className" | "id">;

/**
 * 登录 / 注册共用的输入行。
 *
 * 为什么带图标：这一页只有 2~3 个字段，图标用来做视觉锚点，
 * 让"邮箱 / 密码 / 昵称"在一眼扫过时就能区分，而不是三根一模一样的横线。
 *
 * 无障碍：label 始终可见（不用 placeholder 冒充标签）；
 * 错误与提示各自挂 `aria-describedby`，错误行用 `role="alert"` 播报。
 */
export const AuthField = ({
  label,
  icon: Icon,
  hint,
  error,
  optional = false,
  type = "text",
  ...inputProps
}: AuthFieldProps) => {
  const generatedId = useId();
  const inputId = `auth-${generatedId}`;
  // 错误出现时替代提示文案，所以两个 id 只会生效一个 —— 挂多了会让
  // 读屏把"已隐藏的那条"也念出来。
  const errorId = error ? `${inputId}-error` : undefined;
  const hintId = !error && hint ? `${inputId}-hint` : undefined;
  const [revealed, setRevealed] = useState(false);
  const isPassword = type === "password";

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <label
          htmlFor={inputId}
          className="text-sm leading-5 font-medium text-foreground"
        >
          {label}
        </label>
        {optional ? (
          <span className="text-xs leading-4 text-muted-foreground">
            选填
          </span>
        ) : null}
      </div>

      <div className="group relative">
        <Icon
          className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground/70 transition-colors group-focus-within:text-auth-teal"
          aria-hidden="true"
        />
        <input
          {...inputProps}
          id={inputId}
          type={isPassword && revealed ? "text" : type}
          aria-invalid={error ? true : undefined}
          aria-describedby={
            [hintId, errorId].filter(Boolean).join(" ") || undefined
          }
          className={cn(
            "h-10 w-full rounded-lg border border-border bg-white pr-3 pl-10 text-sm text-foreground",
            "shadow-[inset_0_1px_2px_rgb(19_51_107/0.04)] transition",
            "placeholder:text-muted-foreground/65",
            "hover:border-border-strong/70",
            "focus:border-auth-teal/55 focus:bg-white focus:ring-4 focus:ring-auth-teal/12 focus:outline-none",
            isPassword && "pr-13",
            error &&
              "border-destructive/55 bg-destructive/[0.03] focus:border-destructive/60 focus:ring-destructive/12",
          )}
        />

        {isPassword ? (
          <button
            type="button"
            onClick={() => setRevealed((value) => !value)}
            aria-label={revealed ? "隐藏密码" : "显示密码"}
            aria-pressed={revealed}
            aria-controls={inputId}
            className={cn(
              "absolute top-1/2 right-2 flex size-9 -translate-y-1/2 items-center justify-center rounded-lg",
              "text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
              "focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
            )}
          >
            {revealed ? (
              <EyeOff className="size-[18px]" aria-hidden="true" />
            ) : (
              <Eye className="size-[18px]" aria-hidden="true" />
            )}
          </button>
        ) : null}
      </div>

      {error ? (
        <p id={errorId} role="alert" className="text-[13px] leading-5 text-destructive">
          {error}
        </p>
      ) : hint ? (
        <p id={hintId} className="text-xs leading-4 text-muted-foreground">
          {hint}
        </p>
      ) : null}
    </div>
  );
};
