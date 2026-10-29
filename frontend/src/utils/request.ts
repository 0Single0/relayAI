import axios, {
  type AxiosRequestConfig,
  type InternalAxiosRequestConfig,
} from "axios";
import { clearAuthSession, getAuthToken } from "@/utils/auth";

const instance = axios.create({
  baseURL: import.meta.env.VITE_BASE_URL || "/api/v1",
  timeout: 60000,
});

const successCodes = new Set([0, 200]);

export type ResponseDataType<T = unknown> = {
  code: number;
  success: boolean;
  msg?: string;
  data?: T;
};

export type ApiError = Error & {
  code?: number;
};

export const isSuccessCode = (code?: number) =>
  typeof code === "number" && successCodes.has(code);

const startInterceptors = (
  config: InternalAxiosRequestConfig,
): InternalAxiosRequestConfig => {
  const token = getAuthToken();
  if (token) {
    config.headers.authorization = token;
  }

  const isFormData =
    typeof FormData !== "undefined" && config.data instanceof FormData;
  if (!isFormData && !config.headers["content-type"]) {
    config.headers["content-type"] = "application/json";
  }

  return config;
};

instance.interceptors.request.use(startInterceptors, (error) =>
  Promise.reject(error),
);

instance.interceptors.response.use(
  (response) => {
    const payload = response.data as ResponseDataType | undefined;

    if (
      payload &&
      typeof payload.code === "number" &&
      !isSuccessCode(payload.code)
    ) {
      if (payload.code === 401) {
        clearAuthSession();
      }

      const error = new Error(payload.msg || "请求失败") as ApiError;
      error.code = payload.code;
      return Promise.reject(error);
    }

    return response.data;
  },
  (error) => {
    if (axios.isAxiosError(error) && error.response?.status === 401) {
      clearAuthSession();
    }

    return Promise.reject(error);
  },
);

export const getRequestErrorMessage = (
  error: unknown,
  fallback = "网络异常，请稍后重试",
) => {
  if (axios.isAxiosError(error)) {
    const payload = error.response?.data as ResponseDataType | undefined;
    if (payload?.msg) {
      return payload.msg;
    }
    if (error.code === "ECONNABORTED" || error.code === "ETIMEDOUT") {
      return "请求超时，请检查上游服务后重试";
    }
    const status = error.response?.status;
    if (status === 404) return "接口不存在（HTTP 404），请确认后端已更新并重启";
    if (status === 502 || status === 503) return `服务暂时不可用（HTTP ${status}），请检查后端和网关是否正常运行`;
    if (status === 504) return "网关请求超时（HTTP 504），请稍后重试";
    if (status === 401) return "登录已过期，请重新登录";
    if (status === 403) return "没有权限执行此操作";
    if (status) return `请求失败（HTTP ${status}），请稍后重试`;
    return "无法连接服务，请检查网络和后端是否正常运行";
  }

  if (error instanceof Error && error.message) {
    return error.message;
  }

  return fallback;
};

export const APIRequest = <T = unknown, U = unknown>(
  config: AxiosRequestConfig<U>,
) => instance.request<T, ResponseDataType<T>>(config);
