import { useEffect } from "react";

export const usePageMeta = (title: string) => {
  useEffect(() => {
    document.title = title === "relayAI" ? title : `${title} · relayAI`;
  }, [title]);
};
