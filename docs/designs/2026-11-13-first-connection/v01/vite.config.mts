import { fileURLToPath } from "node:url";
import base from "../../../../apps/desktop/vite.config";

const repo = fileURLToPath(new URL("../../../../", import.meta.url));
const desktopModules = `${repo}apps/desktop/node_modules`;
export default {
  ...base,
  root: fileURLToPath(new URL(".", import.meta.url)),
  resolve: {
    alias: [
      { find: "@", replacement: base.resolve?.alias?.["@"] },
      { find: /^react($|\/)/, replacement: `${desktopModules}/react$1` },
      { find: /^react-dom($|\/)/, replacement: `${desktopModules}/react-dom$1` },
      { find: /^@tauri-apps\/api\/(.*)$/, replacement: `${desktopModules}/@tauri-apps/api/$1.js` },
    ],
  },
  optimizeDeps: { include: [] },
  server: { host: "127.0.0.1", port: 5245, strictPort: true, fs: { allow: [repo] } },
  build: { ...base.build, outDir: "/tmp/crossbow-first-connection-v01" },
};
