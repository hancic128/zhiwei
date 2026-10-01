import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import "@/i18n";
import { PrefsProvider } from "@/components/prefs-provider";
import { ToastProvider } from "@/components/ui/toast";
import { applyThemeEarly } from "@/lib/prefs";
import "./index.css";

// 首帧应用主题 / 暗色，避免闪烁
applyThemeEarly();

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: 5000,
      refetchOnWindowFocus: true,
      // 规范 08：失败态常驻并由组件提供「重试」，不静默重试
      retry: false,
      staleTime: 2000,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <PrefsProvider>
        <ToastProvider>
          <BrowserRouter>
            <App />
          </BrowserRouter>
        </ToastProvider>
      </PrefsProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
