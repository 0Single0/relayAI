import { create } from "zustand";
import {
  getMe,
  login as loginApi,
  logout as logoutApi,
  register as registerApi,
  type User,
} from "@/api/auth";
import { clearAuthSession, getAuthToken, setAuthToken } from "@/utils/auth";
import { getRequestErrorMessage } from "@/utils/request";

type AuthState = {
  user: User | null;
  /** 是否已经问过后端。`false` 时界面应显示加载态，不要直接跳登录页。 */
  resolved: boolean;
  /** 正在提交登录/注册。 */
  pending: boolean;
  error: string;
  /** 启动时恢复会话：有令牌就问一次 `/auth/me`。 */
  bootstrap: () => Promise<void>;
  login: (email: string, password: string) => Promise<boolean>;
  register: (email: string, password: string, nickname: string) => Promise<boolean>;
  logout: () => Promise<void>;
  /** 充值后刷新（余额变了，但令牌没变）。 */
  refresh: () => Promise<void>;
  clearError: () => void;
};

/**
 * 登录态。
 *
 * 为什么用 Zustand 而不是 Context：这个状态要被路由守卫、顶栏、
 * 用户中心三处同时读，且登录动作要能改它。用 Context 会变成
 * "每加一个读取点就多一层 provider"，而 Zustand 的选择器天然做到按需订阅。
 *
 * `resolved` 是必需的：首屏拿到令牌后要先问一次 `/auth/me` 确认它还有效。
 * 少了这个标记，界面会在"还没问完"的瞬间就把用户踢去登录页 —— 表现为
 * 刷新页面必定闪一下登录页，非常难受。
 */
export const useAuthStore = create<AuthState>((set, get) => ({
  user: null,
  resolved: false,
  pending: false,
  error: "",

  bootstrap: async () => {
    const token = getAuthToken();
    if (!token) {
      set({ user: null, resolved: true });
      return;
    }

    try {
      const response = await getMe();
      set({ user: response.data ?? null, resolved: true });
    } catch {
      // 令牌过期/被吊销：清掉，让界面回到未登录。
      clearAuthSession();
      set({ user: null, resolved: true });
    }
  },

  login: async (email, password) => {
    set({ pending: true, error: "" });
    try {
      const response = await loginApi({ email, password });
      const payload = response.data;
      if (!payload) {
        set({ error: "登录失败：后端没有返回用户" });
        return false;
      }
      setAuthToken(payload.token);
      set({ user: payload.user, resolved: true });
      return true;
    } catch (requestError) {
      set({ error: getRequestErrorMessage(requestError) });
      return false;
    } finally {
      set({ pending: false });
    }
  },

  register: async (email, password, nickname) => {
    set({ pending: true, error: "" });
    try {
      const response = await registerApi({ email, password, nickname });
      const payload = response.data;
      if (!payload) {
        set({ error: "注册失败：后端没有返回用户" });
        return false;
      }
      setAuthToken(payload.token);
      set({ user: payload.user, resolved: true });
      return true;
    } catch (requestError) {
      set({ error: getRequestErrorMessage(requestError) });
      return false;
    } finally {
      set({ pending: false });
    }
  },

  logout: async () => {
    try {
      await logoutApi();
    } catch {
      // 服务端吊销失败也要清本地：用户点了退出就该退出。
    }
    clearAuthSession();
    set({ user: null, resolved: true, error: "" });
  },

  refresh: async () => {
    if (!get().user) {
      return;
    }
    try {
      const response = await getMe();
      set({ user: response.data ?? null });
    } catch {
      // 刷新失败保持原值：余额显示旧数字，好过突然变成未登录。
    }
  },

  clearError: () => set({ error: "" }),
}));
