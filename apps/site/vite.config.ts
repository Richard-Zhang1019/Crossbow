import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 部署路径可配：
// - GitHub Pages（默认）：/Crossbow/
// - Netlify 等根路径托管：构建时设 VITE_BASE=/
const base = process.env.VITE_BASE ?? "/Crossbow/";

export default defineConfig({
  base,
  plugins: [react()],
  build: {
    target: "es2020",
  },
});
