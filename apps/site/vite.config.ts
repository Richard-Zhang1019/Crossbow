import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// GitHub Pages 项目站点：仓库 Crossbow → 子路径部署
export default defineConfig({
  base: "/Crossbow/",
  plugins: [react()],
  build: {
    target: "es2020",
  },
});
