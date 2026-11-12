import { fileURLToPath } from "node:url";
import preview from "../../2026-11-05-overview-height/v02/vite.config.mts";

const mock = fileURLToPath(new URL("./mock-tauri.ts", import.meta.url));

export default {
  ...preview,
  root: fileURLToPath(new URL(".", import.meta.url)),
  resolve: {
    ...preview.resolve,
    alias: preview.resolve.alias.map((alias) =>
      alias.replacement.endsWith("/mock-tauri.ts") ? { ...alias, replacement: mock } : alias,
    ),
  },
  server: { ...preview.server, port: 5222 },
  build: {
    ...preview.build,
    outDir: "/tmp/crossbow-toolbar-update-v01-dist",
    rollupOptions: { input: ["index.html", "matrix.html"].map(page => fileURLToPath(new URL(page, import.meta.url))) },
  },
};
