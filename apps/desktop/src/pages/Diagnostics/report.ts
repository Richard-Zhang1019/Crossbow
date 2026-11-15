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
