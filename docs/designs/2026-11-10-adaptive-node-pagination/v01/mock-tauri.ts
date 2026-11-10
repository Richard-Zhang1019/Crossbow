import { invoke as baseInvoke } from "../../2026-11-06-toolbar-update/v01/mock-tauri";
export * from "../../2026-11-06-toolbar-update/v01/mock-tauri";

const params = new URLSearchParams(location.search);
const english = params.has("en");
const automatic = english ? "Auto select" : "自动选择";
const cities = ["🇯🇵 Tokyo", "🇭🇰 Hong Kong", "🇸🇬 Singapore", "🇺🇸 Seattle"];
const nodes = Array.from({ length: params.has("few") ? 2 : 13 }, (_, index) => ({
  name: index === 0 ? automatic : `${cities[(index - 1) % cities.length]} · ${index}`,
  kind: index === 0 ? "URLTest" : index % 3 === 0 ? "VLESS" : "Hysteria2",
  delay_ms: index % 3 === 0 ? null : 76 + 19 * index,
  alive: index % 3 !== 0,
}));

export const getVersion = async () => "0.14.1-pagination-preview";

export async function invoke(command: string, args: Record<string, unknown> = {}) {
  if (command === "proxies_snapshot") {
    return [
      { name: english ? "Node selection" : "节点选择", kind: "Selector", now: automatic, nodes },
      { name: automatic, kind: "URLTest", now: nodes[1].name, nodes: nodes.slice(1) },
    ];
  }
  if (command === "app_version") return getVersion();
  return baseInvoke(command, args);
}
