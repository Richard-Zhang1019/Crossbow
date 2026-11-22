import { invoke } from "@tauri-apps/api/core";
import type { NodeDelayContext } from "./ipc";
export type ProbeKind = "connectivity" | "throughput";
export interface ProbeCheck { class: "ok" | "tunnel" | "https_unverified" | "udp_unverified" | "unsupported" | "not_tested"; elapsedMs: number; packets: number; }
export interface ProbeTransfer { class: "ok" | "incomplete" | "not_tested"; bytes: number; elapsedMs: number; limitBytes: number; mbps: number | null; }
export interface ProbeReport { kind: ProbeKind; atMs: number; elapsedMs: number; tcp: ProbeCheck; https: ProbeCheck; udp: ProbeCheck; download: ProbeTransfer; upload: ProbeTransfer; }
export interface ProbeSnapshot { id: string; node: string; kind: ProbeKind; state: "running" | "cancelling" | "cancelled" | "completed" | "expired" | "failed"; phase: "preparing" | "tcp" | "https" | "udp" | "download" | "upload" | "finished"; report: ProbeReport | null; error: string | null; }
export const nodeProbes = {
  start: (node: string, context: NodeDelayContext, kind: ProbeKind) => invoke<string>("start_node_probe", { node, context, kind }),
  snapshot: (id: string, context: NodeDelayContext) => invoke<ProbeSnapshot | null>("node_probe_snapshot", { id, context }),
  cancel: (id: string) => invoke<void>("cancel_node_probe", { id }),
  history: (node: string, context: NodeDelayContext) => invoke<ProbeReport[]>("node_probe_history", { node, context }),
};
