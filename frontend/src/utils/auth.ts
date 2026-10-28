const AUTH_TOKEN_KEY = "relayai.auth.token";
const REMEMBERED_CREDENTIALS_KEY = "relayai.auth.remembered";

const getStorage = (): Storage | null => {
  if (typeof window === "undefined") {
    return null;
  }

  return window.localStorage;
};

export const getAuthToken = () => getStorage()?.getItem(AUTH_TOKEN_KEY) ?? "";

export const setAuthToken = (token: string) => {
  getStorage()?.setItem(AUTH_TOKEN_KEY, token);
};

export const clearAuthSession = () => {
  getStorage()?.removeItem(AUTH_TOKEN_KEY);
};

/** 「记住密码」保存的凭据。 */
export type RememberedCredentials = {
  email: string;
  password: string;
};

export const clearRememberedCredentials = () => {
  getStorage()?.removeItem(REMEMBERED_CREDENTIALS_KEY);
};

/**
 * 写入「记住密码」。
 *
 * ⚠️ 安全边界必须说清楚：这个功能按字面意思实现 —— **密码以明文**存放在
 * 本机 localStorage。能执行本站脚本的东西（浏览器扩展、XSS、共用这台电脑的
 * 其他人）都能直接读到它。
 *
 * 所以这里**不做** base64、混淆或前端"加密"：密钥同样得放在本地，
 * 那种做法只会让用户误以为密码被保护了，反而更危险。
 *
 * 真正安全的做法是后端下发长效令牌（"记住我"而不是"记住密码"），
 * 当前后端没有这个能力；在它出现之前，明文的取舍由用户自己决定。
 */
export const setRememberedCredentials = (credentials: RememberedCredentials) => {
  getStorage()?.setItem(REMEMBERED_CREDENTIALS_KEY, JSON.stringify(credentials));
};

/** 读取「记住密码」；内容损坏时顺手清掉，避免坏数据把登录页卡住。 */
export const getRememberedCredentials = (): RememberedCredentials | null => {
  const raw = getStorage()?.getItem(REMEMBERED_CREDENTIALS_KEY);
  if (!raw) {
    return null;
  }

  try {
    const parsed = JSON.parse(raw) as Partial<RememberedCredentials>;
    if (
      typeof parsed.email === "string" &&
      typeof parsed.password === "string" &&
      parsed.email !== "" &&
      parsed.password !== ""
    ) {
      return { email: parsed.email, password: parsed.password };
    }
  } catch {
    // 落到下面统一清理。
  }

  clearRememberedCredentials();
  return null;
};
