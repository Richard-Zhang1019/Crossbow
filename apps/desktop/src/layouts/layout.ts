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
