/** Match the card grid's minimum widths and gaps without stretching sparse results. */
export function nodeResultHeight(
  length: number,
  width: number,
): number {
  const columns = Math.max(1, Math.min(4, Math.floor((width + 10) / 210)));
  const row = 76, limit = 4;
  const rows = Math.max(1, Math.ceil(length / columns));
  return Math.min(rows, limit) * (row + 8) - 8 + (rows > limit ? 40 : 0);
}
export function friendlyKind(kind: string, t: (key: string) => string): string {
  const group: Record<string, string> = {
    Selector: "layout.manual",
    URLTest: "layout.auto",
    Fallback: "layout.fallback",
    LoadBalance: "layout.balance",
    Direct: "layout.direct",
    Reject: "layout.reject",
  };
  if (group[kind]) return t(group[kind]);
  const protocol: Record<string, string> = {
    vless: "VLESS",
    vmess: "VMess",
    hysteria2: "Hysteria 2",
    hysteria: "Hysteria",
    shadowsocks: "Shadowsocks",
    trojan: "Trojan",
    tuic: "TUIC",
    http: "HTTP",
    socks5: "SOCKS5",
  };
  return protocol[kind.toLowerCase()] ?? kind;
}
