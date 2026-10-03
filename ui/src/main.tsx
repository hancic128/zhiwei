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

// Apply theme / dark mode on first frame to avoid flash
applyThemeEarly();

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchInterval: 5000,
      refetchOnWindowFocus: true,
      // Spec 08: failure state stays resident and components provide a "retry" button, no silent retries
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
