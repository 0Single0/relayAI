import { useCallback, useMemo, useState, type FormEvent } from "react";
import {
  ArrowRight,
  CircleAlert,
  LoaderCircle,
  Lock,
  Mail,
  UserRound,
} from "lucide-react";
import { useLocation, useNavigate } from "react-router-dom";
import { AuthBackdrop } from "@/pages/Auth/components/AuthBackdrop";
import { AuthBrand } from "@/pages/Auth/components/AuthBrand";
import { AuthField } from "@/pages/Auth/components/AuthField";
import { usePageMeta } from "@/hooks/usePageMeta";
import { useAuthStore } from "@/store/auth";
import {
  clearRememberedCredentials,
  getRememberedCredentials,
  setRememberedCredentials,
} from "@/utils/auth";
import { cn } from "@/lib/utils";

type Mode = "login" | "register";

type FieldErrors = {
  email?: string;
  password?: string;
  nickname?: string;
};

const EMAIL_PATTERN = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/**
 * 登录 / 注册页。
 *
 * 一个页面两种模式（切换而不是两个路由）：这两个动作的字段几乎重合，
 * 拆成两页只会让用户在"我该点哪个"上多犹豫一次。
 *
 * 视觉：弥散渐变背景 + 居中卡片，配色与光斑见 styles/index.css 的 --auth-*。
 * 第三方登录（微信等）没有接入，所以不画分隔线和按钮 —— 不存在的入口
 * 比没有入口更糟。
 *
 * 第一个注册的用户自动成为管理员（后端判定），注册模式里要说清楚。
 */
export const LoginPage = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const login = useAuthStore((state) => state.login);
  const register = useAuthStore((state) => state.register);
  const pending = useAuthStore((state) => state.pending);
  const serverError = useAuthStore((state) => state.error);
  const clearError = useAuthStore((state) => state.clearError);

  const [mode, setMode] = useState<Mode>("login");
  // 初始化时同步读取：记住密码的功能价值就在于"打开就能直接登录"，
  // 放到 effect 里晚一帧填入会让输入框先闪一下空值。
  const [remembered] = useState(getRememberedCredentials);
  const [email, setEmail] = useState(remembered?.email ?? "");
  const [password, setPassword] = useState(remembered?.password ?? "");
  const [rememberPassword, setRememberPassword] = useState(remembered !== null);
  const [nickname, setNickname] = useState("");
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});

  const isRegister = mode === "register";
  usePageMeta(isRegister ? "注册" : "登录");

  /**
   * 登录成功后的落点：优先回到被守卫拦下来的那一页。
   * 只接受站内路径，避免 `state.from` 被构造成外链。
   */
  const redirectTo = useMemo(() => {
    const from = (location.state as { from?: unknown } | null)?.from;
    return typeof from === "string" &&
      from.startsWith("/") &&
      !from.startsWith("//") &&
      from !== "/login"
      ? from
      : "/dashboard";
  }, [location.state]);

  const clearFieldError = (key: keyof FieldErrors) => {
    setFieldErrors((current) =>
      current[key] ? { ...current, [key]: undefined } : current,
    );
  };

  const handleEmailChange = (value: string) => {
    setEmail(value);
    clearFieldError("email");
    if (serverError) {
      clearError();
    }
  };

  const handlePasswordChange = (value: string) => {
    setPassword(value);
    clearFieldError("password");
    if (serverError) {
      clearError();
    }
  };

  const handleNicknameChange = (value: string) => {
    setNickname(value);
    clearFieldError("nickname");
    if (serverError) {
      clearError();
    }
  };

  const switchMode = useCallback(
    (next: Mode) => {
      setMode(next);
      /*
       * 密码不跨模式保留：登录密码拿去注册会被后端的 8 位规则直接打回。
       * 但切回登录时，若勾选着"记住密码"且本地有记录，就把密码还原回去 ——
       * 否则会出现"勾选框还打着勾、密码框却是空的"这种自相矛盾的状态。
       */
      setPassword(next === "login" && rememberPassword ? (remembered?.password ?? "") : "");
      setFieldErrors({});
      clearError();
    },
    [clearError, rememberPassword, remembered],
  );

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (pending) {
      return;
    }

    const trimmedEmail = email.trim();
    const nextErrors: FieldErrors = {};

    if (!trimmedEmail) {
      nextErrors.email = "请输入邮箱";
    } else if (!EMAIL_PATTERN.test(trimmedEmail)) {
      nextErrors.email = "邮箱格式不正确";
    }

    if (!password) {
      nextErrors.password = "请输入密码";
    } else if (isRegister && password.length < 8) {
      nextErrors.password = "密码至少 8 位";
    } else if (password.length > 128) {
      nextErrors.password = "密码最多 128 位";
    }

    if (nickname.trim().length > 32) {
      nextErrors.nickname = "昵称最多 32 个字符";
    }

    if (Object.keys(nextErrors).length > 0) {
      setFieldErrors(nextErrors);
      return;
    }

    setFieldErrors({});
    const ok = isRegister
      ? await register(trimmedEmail, password, nickname.trim())
      : await login(trimmedEmail, password);

    if (!ok) {
      // 登录失败**不写**凭据：把打错的密码记下来，下次打开就是"自动填一个错的"，
      // 用户还得先清掉才能重输。失败时保留上一次成功的记录不动。
      return;
    }

    /*
     * 只在登录成功后才落盘：注册流程不记忆，避免刚设的新密码被静默存下来。
     * 勾选状态决定写还是清 —— 取消勾选要立刻抹掉旧记录，
     * 否则用户会以为"取消了"，下次打开密码还在。
     */
    if (isRegister) {
      clearRememberedCredentials();
    } else if (rememberPassword) {
      setRememberedCredentials({ email: trimmedEmail, password });
    } else {
      clearRememberedCredentials();
    }

    navigate(redirectTo, { replace: true });
  };

  return (
    <div className="relative flex min-h-dvh flex-col overflow-x-hidden">
      <AuthBackdrop />

      <main className="relative z-10 flex flex-1 items-center justify-center px-4 py-8 sm:px-6 sm:py-10">
        <div className="auth-rise auth-card">
          <section aria-labelledby="auth-brand-title">
            <AuthBrand />

            <form onSubmit={submit} className="mt-14 flex flex-col gap-6" noValidate>
              {isRegister ? (
                <AuthField
                  label="昵称"
                  optional
                  icon={UserRound}
                  value={nickname}
                  onChange={(event) => handleNicknameChange(event.target.value)}
                  placeholder="留空则用邮箱前缀"
                  autoComplete="nickname"
                  maxLength={32}
                  disabled={pending}
                />
              ) : null}

              <AuthField
                label="邮箱"
                icon={Mail}
                type="email"
                value={email}
                onChange={(event) => handleEmailChange(event.target.value)}
                placeholder="you@example.com"
                autoComplete={isRegister ? "email" : "username"}
                inputMode="email"
                autoCapitalize="none"
                spellCheck={false}
                required
                disabled={pending}
                error={fieldErrors.email}
              />

              <AuthField
                label="密码"
                icon={Lock}
                type="password"
                value={password}
                onChange={(event) => handlePasswordChange(event.target.value)}
                placeholder={isRegister ? "至少 8 位字符" : "请输入密码"}
                autoComplete={isRegister ? "new-password" : "current-password"}
                required
                disabled={pending}
                error={fieldErrors.password}
                hint={isRegister ? "至少 8 位，最多 128 位" : undefined}
              />

              {serverError ? (
                <div
                  role="alert"
                  className="flex items-start gap-2 rounded-xl border border-destructive/20 bg-destructive/[0.06] px-3.5 py-3 text-[13px] leading-5 text-destructive"
                >
                  <CircleAlert
                    className="mt-0.5 size-4 shrink-0"
                    aria-hidden="true"
                  />
                  <span className="min-w-0">{serverError}</span>
                </div>
              ) : null}

              {/*
                只在登录模式出现：注册时谈"记住密码"没有意义（密码刚设完），
                而且会诱导用户把新密码留在共享机器上。
              */}
              {!isRegister ? (
                <label
                  htmlFor="remember-password"
                  className="group -mt-1 flex w-fit items-center gap-2.5 text-[13px] leading-5 text-muted-foreground select-none"
                >
                  <input
                    id="remember-password"
                    type="checkbox"
                    checked={rememberPassword}
                    onChange={(event) => {
                      const next = event.target.checked;
                      setRememberPassword(next);
                      // 取消勾选立即清除已保存的密码，不留"看起来取消了、其实还在"的窗口。
                      if (!next) {
                        clearRememberedCredentials();
                      }
                    }}
                    disabled={pending}
                    className={cn(
                      // 用原生复选框 + accent-color：自绘一个 svg 对勾要处理
                      // 任意值 URL 的转义（实测按引号转义后 Tailwind 生成不出规则，
                      // 渲染成一个白方块），原生控件的外观在三大浏览器里已经够用。
                      "size-4 shrink-0 cursor-pointer accent-primary",
                      "focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-offset-2 focus-visible:outline-none",
                      "disabled:cursor-not-allowed disabled:opacity-60",
                    )}
                  />
                  <span className="transition-colors group-hover:text-foreground">
                    记住密码
                  </span>
                </label>
              ) : null}

              <button
                type="submit"
                disabled={pending}
                aria-busy={pending || undefined}
                className={cn(
                  "group mt-1 inline-flex h-12 w-full items-center justify-center gap-2 rounded-xl",
                  "bg-primary text-[15px] font-medium text-primary-foreground",
                  "shadow-[0_14px_28px_-16px_rgb(51_112_255/0.95)] transition-all",
                  "hover:bg-primary/92 hover:shadow-[0_16px_32px_-16px_rgb(51_112_255/0.9)]",
                  "active:translate-y-px disabled:pointer-events-none disabled:opacity-60",
                  "focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
                )}
              >
                {pending ? (
                  <LoaderCircle
                    className="size-[18px] animate-spin motion-reduce:animate-none"
                    aria-hidden="true"
                  />
                ) : null}
                {pending
                  ? isRegister
                    ? "正在创建..."
                    : "正在登录..."
                  : isRegister
                    ? "注册并登录"
                    : "登录"}
                {!pending ? (
                  <ArrowRight
                    className="size-[18px] transition-transform group-hover:translate-x-0.5"
                    aria-hidden="true"
                  />
                ) : null}
              </button>
            </form>

            <div className="mt-6 flex items-center justify-center gap-1.5 text-[13px] text-muted-foreground">
              <span>{isRegister ? "已经有账号？" : "还没有账号？"}</span>
              <button
                type="button"
                onClick={() => switchMode(isRegister ? "login" : "register")}
                className={cn(
                  "rounded-md px-1 py-0.5 font-medium text-primary transition-colors",
                  "hover:text-accent-foreground hover:underline hover:underline-offset-4",
                  "focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:outline-none",
                  "disabled:pointer-events-none disabled:opacity-60",
                )}
                disabled={pending}
              >
                {isRegister ? "直接登录" : "立即注册"}
              </button>
            </div>
          </section>
        </div>
      </main>
    </div>
  );
};
