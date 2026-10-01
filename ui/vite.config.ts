import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 1200,
    rollupOptions: {
      output: {
        manualChunks: {
          echarts: ["echarts"],
          vendor: ["react", "react-dom", "react-router-dom", "@tanstack/react-query"],
        },
      },
    },
  },
  // Dev proxy so `npm run dev` can talk to a locally running monitor.
  server: {
    proxy: {
      "/v1": { target: "https://127.0.0.1:8443", changeOrigin: true, secure: false },
      "/healthz": { target: "https://127.0.0.1:8443", changeOrigin: true, secure: false },
    },
  },
});
