import type { NodeDelayOutcome, NodeView } from "@/services/ipc";

export type NodeDelayDisplay = NodeDelayOutcome & { previousMs?: number | null };

export function nodeDelayValue(node: NodeView, outcome?: NodeDelayDisplay): number | null {
  if (outcome?.class === "ok") return outcome.delayMs;
  if (outcome?.class === "timeout") return 0;
  return outcome?.previousMs === undefined ? node.delay_ms : outcome.previousMs;
}

export function comparableNodeDelay(node: NodeView, outcome?: NodeDelayDisplay): number {
  const delay = nodeDelayValue(node, outcome);
  return !outcome?.reviewed && delay != null && delay > 0 && (outcome?.class === "ok" || node.alive)
    ? delay
    : Number.MAX_SAFE_INTEGER;
}

export const NODE_DELAY_CONCURRENCY = 10;

export async function runNodeDelayQueue(
  nodes: string[],
  deadline: number,
  current: () => boolean,
  measure: (node: string) => Promise<NodeDelayOutcome | undefined>,
  apply: (node: string, outcome: NodeDelayOutcome) => void,
) {
  let next = 0;
  let invalidated = false;
  const worker = async () => {
    while (!invalidated && next < nodes.length && current() && performance.now() < deadline) {
      const node = nodes[next++];
      const outcome = await measure(node);
      if (outcome?.class === "cancelled") invalidated = true;
      if (outcome && !invalidated && current()) apply(node, outcome);
    }
  };
  await Promise.all(Array.from({ length: Math.min(nodes.length, NODE_DELAY_CONCURRENCY) }, worker));
  return invalidated;
}
