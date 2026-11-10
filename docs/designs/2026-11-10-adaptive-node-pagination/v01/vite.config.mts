import { fileURLToPath } from "node:url";
import preview from "../../2026-11-06-toolbar-update/v01/vite.config.mts";

const mock = fileURLToPath(new URL("./mock-tauri.ts", import.meta.url));

export default {
  ...preview,
  root: fileURLToPath(new URL(".", import.meta.url)),
  resolve: {
    ...preview.resolve,
    alias: [
      { find: "@", replacement: fileURLToPath(new URL("../../../../apps/desktop/src", import.meta.url)) },
      ...preview.resolve.alias.map((alias) =>
        alias.replacement.endsWith("/mock-tauri.ts") ? { ...alias, replacement: mock } : alias,
      ),
    ],
  },
  server: { ...preview.server, port: 5224 },
  build: {
    ...preview.build,
    outDir: "/tmp/crossbow-adaptive-node-pagination-v01",
    rollupOptions: { input: fileURLToPath(new URL("./index.html", import.meta.url)) },
  },
};
