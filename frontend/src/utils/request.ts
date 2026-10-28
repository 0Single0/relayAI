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

    return fallback;
  }

  if (error instanceof Error && error.message) {
    return error.message;
  }

  return fallback;
};

export const APIRequest = <T = unknown, U = unknown>(
  config: AxiosRequestConfig<U>,
) => instance.request<T, ResponseDataType<T>>(config);
