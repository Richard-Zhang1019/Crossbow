import type { CheckResult, DiagnosticReport, FaultEvidence, FaultEvidenceProbe } from "@/services/ipc";

export function faultProbeText(probe: FaultEvidenceProbe, text: (key: string) => string) {
  const known = ["ok", "timeout", "connect", "tls", "dns", "http_status", "unknown"];
  const kind = known.includes(probe.class) ? probe.class : "unknown";
  const status = probe.httpStatus;
  const showStatus = (kind === "ok" || kind === "http_status") && status != null
    && Number.isInteger(status) && status >= 100 && status <= 599;
  return text(`fault.class.${kind}`) + (showStatus ? ` ${status}` : "");
}

export function faultEvidenceState(fault: FaultEvidence) {
  if (fault.recovery) return "resolved";
  if (fault.superseded) return "superseded";
  if (fault.resolvedAtMs != null) return fault.legacy ? "legacyResolved" : "reachable";
  return "ongoing";
}

export function probeOk(probe: FaultEvidenceProbe | null | undefined) {
  return !!probe && probe.class === "ok";
}

function layerOk(layer: { primary: FaultEvidenceProbe; review?: FaultEvidenceProbe | null }) {
  return probeOk(layer.primary) && (!layer.review || probeOk(layer.review));
}

/** 五个探测层的成败统计（故障折叠条摘要行用）。 */
export function faultLayerStats(fault: FaultEvidence) {
  const layerOkFlags = [
    fault.mixedPortOk,
    probeOk(fault.controller),
    layerOk(fault.directCn),
    layerOk(fault.directGlobal),
    layerOk(fault.proxyHttps),
  ];
  return { total: layerOkFlags.length, failed: layerOkFlags.filter(pass => !pass).length };
}

/**
 * 「接口 → 端点」观测按端点去重；全部指向同一端点时返回按接口名聚合的分组，
 * 解析失败或存在多目标时返回 null（调用方回退到原始列表）。
 */
export function dedupeObservedTargets(observed: string[]) {
  const parsed: { iface: string; target: string }[] = [];
  for (const entry of observed) {
    const sep = entry.indexOf(" → ");
    if (sep < 0) return null;
    const target = entry.slice(sep + 3);
    if (!target) return null;
    parsed.push({ iface: entry.slice(0, sep), target });
  }
  if (!parsed.length || new Set(parsed.map(item => item.target)).size !== 1) return null;
  const groups = new Map<string, number>();
  for (const item of parsed) groups.set(item.iface, (groups.get(item.iface) ?? 0) + 1);
  return [...groups].map(([iface, count]) => ({ iface, target: parsed[0].target, count }));
}

const faultStateText: Record<ReturnType<typeof faultEvidenceState>, string> = {
  resolved: "recovered (verified)",
  superseded: "context changed; recovery unconfirmed",
  legacyResolved: "recovery recorded in legacy format",
  reachable: "reachable during initial observation",
  ongoing: "ongoing",
};

/** 白名单摘要：不导出 profile、网络指纹、服务名称、代理地址或任意原始错误。 */
export function faultEvidenceText(fault: FaultEvidence) {
  const text = (key: string) => key.replace("fault.class.", "");
  const layer = (value: FaultEvidence["proxyHttps"]) =>
    `primary=${faultProbeText(value.primary, text)}` +
    (value.review ? `; review=${faultProbeText(value.review, text)}` : "");
  return [
    "System proxy fault evidence (explicit mixed-port HTTPS; node unknown)",
    `Started: ${new Date(fault.startedAtMs).toISOString()}`,
    `State: ${faultStateText[faultEvidenceState(fault)]}`,
    `Mixed port: ${fault.mixedPort}; TCP=${fault.mixedPortOk ? "ok" : "connect"}`,
    `Controller: ${faultProbeText(fault.controller, text)}`,
    "Direct baseline: bypassed explicit proxy; physical path unverified",
    `Direct CN: ${layer(fault.directCn)}`,
    `Direct global: ${layer(fault.directGlobal)}`,
    `Proxy HTTPS: ${layer(fault.proxyHttps)}`,
    ...(fault.recovery ? [
      `Recovery: ${new Date(fault.recovery.atMs).toISOString()}`,
      `Recovery probe: ${layer(fault.recovery.proxyHttps)}`,
    ] : []),
    ...(fault.legacy && fault.resolvedAtMs != null && !fault.recovery ? [
      `Legacy recovery timestamp: ${new Date(fault.resolvedAtMs).toISOString()}`,
    ] : []),
    ...(fault.legacy ? ["Note: legacy fault-time proxy samples may have been overwritten"] : []),
  ].join("\n");
}

const MAX_AGE_MS = 60_000;

export type DiagnosisSnapshot = DiagnosticReport & {
  revision: number;
  completedAt: number;
};

export function isStale(report: DiagnosisSnapshot, revision: number, now = Date.now()) {
  return report.revision !== revision || now - report.completedAt >= MAX_AGE_MS;
}

export function remainingAge(report: DiagnosisSnapshot) {
  return Math.max(0, report.completedAt + MAX_AGE_MS - Date.now());
}

const failureDescriptions: Record<string, string> = {
  core_running: "Core process unavailable",
  controller: "Core controller did not respond",
  config_renders: "Active configuration could not be rendered",
  listener: "Local mixed port could not be reached",
  sysproxy: "Takeover settings or TUN could not be verified",
  proxied_traffic: "Explicit HTTP proxy connectivity check failed",
  dns_direct: "System DNS lookup failed",
};

export function guideUrl(id: string, lang: "en" | "zh", context: DiagnosticReport["context"]) {
  const anchor = {
    core_running: "engines",
    controller: "engines",
    config_renders: "quick-start",
    listener: "ports",
    sysproxy: context.os === "windows" && (context.tun === "inactive" || context.tun === "unknown")
      ? "windows" : "proxy-vs-tun",
    proxied_traffic: "proxy-vs-tun",
  } as Record<string, string>;
  const section = anchor[id] ?? "troubleshooting";
  const locale = lang === "zh" ? "/zh" : "";
  return `https://crossbow-client.netlify.app${locale}/guide#${section}`;
}

export function failedChecks(rows: CheckResult[]) {
  return rows.filter(row => !row.ok && !row.skipped);
}

export function shareText(report: DiagnosisSnapshot, appVersion: string, stale: boolean) {
  const context = report.context;
  const failed = failedChecks(report.rows);
  return [
    `Crossbow ${appVersion || "unknown"}`,
    `System: ${context.os} ${context.os_version ?? "unknown"}`,
    `Core: ${context.engine} ${context.core_version ?? "unknown"} (${context.core_state})`,
    `System proxy: ${context.proxy}`,
    `TUN: ${context.tun}`,
    `Mixed port: ${context.mixed_port}`,
    `Checked at: ${new Date(report.completedAt).toISOString()}`,
    `Result: ${stale ? "historical — check again" : failed.length ? "issues found" : "passed"}`,
    ...failed.map(row => `- ${failureDescriptions[row.id] ?? "Unknown check"}`),
  ].join("\n");
}
