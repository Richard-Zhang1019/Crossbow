import { defineConfig } from "astro/config";
import react from "@astrojs/react";
import sitemap from "@astrojs/sitemap";

// 部署路径可配：
// - Netlify（默认）：根路径
// - GitHub Pages：构建时设 SITE_BASE=/Crossbow/
const base = process.env.SITE_BASE ?? "/";
const site = process.env.SITE_URL ?? "https://crossbow-client.netlify.app";

export default defineConfig({
  site,
  base,
  output: "static",
  trailingSlash: "never",
  integrations: [react(), sitemap()],
});
