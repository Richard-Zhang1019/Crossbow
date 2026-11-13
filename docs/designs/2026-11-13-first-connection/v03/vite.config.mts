import { fileURLToPath } from "node:url";
import base from "../v02/vite.config.mts";

export default {
  ...base,
  root: fileURLToPath(new URL(".", import.meta.url)),
  server: { ...base.server, port: 5248 },
  build: { ...base.build, outDir: "/tmp/crossbow-first-connection-v03" },
};
