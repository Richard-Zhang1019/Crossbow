import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri 前端构建配置：固定端口、排除浏览器缓存干扰。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  resolve: {
    // 防止 dev 预打包出现两份 React（Invalid hook call 白屏）
    dedupe: ["react", "react-dom"],
  },
  optimizeDeps: {
    // 显式列出全部依赖，确保首轮一次性完成预打包。
    // 分轮优化会产生多套 ?v=hash，与浏览器 immutable 缓存叠加时
    // 会出现两份 React 实例（Invalid hook call → 白屏）。
    include: [
      "react",
      "react-dom",
      "react-dom/client",
      "react/jsx-runtime",
      "react/jsx-dev-runtime",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
    ],
  },
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "safari16",
    minify: "esbuild",
    sourcemap: false,
  },
});
