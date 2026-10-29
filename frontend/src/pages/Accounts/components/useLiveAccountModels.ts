import { useCallback, useEffect, useRef, useState } from "react";
import { getLiveAccountModels, type LiveAccountModels } from "@/api/accounts/liveModels";
import { getRequestErrorMessage } from "@/utils/request";

export const useLiveAccountModels = (accountId: number) => {
  const [data, setData] = useState<LiveAccountModels | null>(null);
  const [isLoading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const request = useRef<AbortController | null>(null);

  const refresh = useCallback(async () => {
    if (request.current) return;
    const controller = new AbortController();
    request.current = controller;
    setLoading(true);
    setError("");
    setData(null);
    try {
      const response = await getLiveAccountModels(accountId, controller.signal);
      if (!controller.signal.aborted) {
        if (!response.data) throw new Error("上游没有返回账号模型数据");
        setData(response.data);
      }
    } catch (requestError) {
      if (!controller.signal.aborted) setError(getRequestErrorMessage(requestError));
    } finally {
      if (request.current === controller) {
        request.current = null;
        if (!controller.signal.aborted) setLoading(false);
      }
    }
  }, [accountId]);

  useEffect(() => {
    void refresh();
    const refreshVisible = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    const interval = window.setInterval(refreshVisible, 30_000);
    window.addEventListener("focus", refreshVisible);
    document.addEventListener("visibilitychange", refreshVisible);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshVisible);
      document.removeEventListener("visibilitychange", refreshVisible);
      request.current?.abort();
      request.current = null;
    };
  }, [refresh]);

  return { data, isLoading, error, refresh };
};
