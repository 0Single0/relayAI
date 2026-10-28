import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "react-router-dom";
import { ToastViewport } from "@/components/common/Toast";
import { router } from "@/routes";
import "@/styles/index.css";

const rootElement = document.getElementById("root");

if (!rootElement) {
  throw new Error("Root element #root was not found");
}

createRoot(rootElement).render(
  <StrictMode>
    {/*
      Toast 视口挂在路由之外：它不随页面切换卸载，
      否则"保存失败"的提示会在用户点导航的瞬间消失。
    */}
    <RouterProvider router={router} />
    <ToastViewport />
  </StrictMode>,
);
