import type { DnsPathResult, DnsProbeResult } from "@/services/ipc";

/** 只保留本次会话最近几条：有界历史，可清除，不落盘。 */
export const DNS_HISTORY_LIMIT = 5;

export interface DnsHistoryEntry {
  probe: DnsProbeResult;
  /** 发起检查时的配置序号；与之不同即说明结果可能已过期。 */
  revision: number;
}

const PATH_STATES = [
  "ok", "no_record", "nxdomain", "servfail", "refused", "timeout", "error", "unavailable",
] as const;

const VERDICTS = [
  "both_ok", "system_only", "kernel_only", "both_fail", "kernel_unavailable",
] as const;

export type DnsPathState = (typeof PATH_STATES)[number];
export type DnsVerdict = (typeof VERDICTS)[number];

/** 未知状态按失败展示，不冒充成功。 */
export function pathStateId(state: string): DnsPathState {
  return (PATH_STATES as readonly string[]).includes(state) ? (state as DnsPathState) : "error";
}

export function verdictId(verdict: string): DnsVerdict {
  return (VERDICTS as readonly string[]).includes(verdict) ? (verdict as DnsVerdict) : "both_fail";
}

/** ok / warn / bad：只有两条路径都成功才算 ok，未观测不算通过。 */
export function verdictTone(verdict: string): "ok" | "warn" | "bad" {
  switch (verdictId(verdict)) {
    case "both_ok": return "ok";
    case "kernel_only":
    case "kernel_unavailable": return "warn";
    default: return "bad";
  }
}

export function pushHistory(
  history: DnsHistoryEntry[],
  entry: DnsHistoryEntry,
  limit = DNS_HISTORY_LIMIT,
) {
  return [entry, ...history].slice(0, Math.max(1, limit));
}

function pathSummary(path: DnsPathResult) {
  return [
    pathStateId(path.state),
    `${path.ms}ms`,
    path.rcode != null ? `rcode=${path.rcode}` : null,
    path.answers.length ? `answers=${path.answers.join(",")}` : null,
    path.ttl != null ? `ttl=${path.ttl}` : null,
  ].filter(Boolean).join(" ");
}

/**
 * 白名单摘要：只含用户输入的域名、公开应答地址与路径结论。
 * 不含订阅、配置内容、控制器端口/密钥或任意原始错误（错误另走脱敏通知）。
 */
export function probeSummary(probe: DnsProbeResult) {
  const engine = [
    probe.engine,
    probe.core_version ? `v${probe.core_version.replace(/^v/, "")}` : null,
    probe.core_pid != null ? `pid=${probe.core_pid}` : null,
  ].filter(Boolean).join(" ");
  return [
    "DNS resolution check",
    `Domain: ${probe.domain} ${probe.record_type}`,
    `At: ${new Date(probe.at_ms).toISOString()}`,
    `Engine: ${engine}`,
    `DNS mode: ${probe.dns_mode}`,
    `System: ${pathSummary(probe.system)}`,
    `Kernel: ${pathSummary(probe.kernel)}`,
    `Verdict: ${probe.verdict}`,
    probe.hosts_override ? "Hosts override: yes" : null,
    `Unknown fields: ${probe.unknown_fields.join(", ")}`,
  ].filter(Boolean).join("\n");
}
