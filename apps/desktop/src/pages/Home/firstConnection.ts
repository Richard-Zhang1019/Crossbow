export type SetupProgress = "pending" | "complete" | null;
const PROGRESS_KEY = "crossbow:first-connection";

export function readSetupProgress(): SetupProgress {
  const value = localStorage.getItem(PROGRESS_KEY);
  return value === "pending" || value === "complete" ? value : null;
}

export function writeSetupProgress(progress: NonNullable<SetupProgress>) {
  localStorage.setItem(PROGRESS_KEY, progress);
}

export function nextSetupStep({
  progress, configured, installed, coreState, takeoverAvailable, takeoverEnabled,
}: {
  progress: SetupProgress;
  configured: boolean;
  installed: boolean;
  coreState: "Stopped" | "Starting" | "Running" | "Crashed";
  takeoverAvailable: boolean;
  takeoverEnabled: boolean;
}) {
  if (!configured) return "import";
  if (progress !== "pending") return null;
  if (coreState === "Starting") return "start";
  if (coreState !== "Running") return installed ? "start" : "download";
  if (takeoverAvailable && !takeoverEnabled) return "takeover";
  return "verify";
}

/** 端口变更后的旧失败不能再引导修改当前端口；不以文案猜测故障原因。 */
export function isPortUnavailable(error: unknown, port: number | undefined): boolean {
  return !!error && typeof error === "object" && "kind" in error &&
    error.kind === "portUnavailable" && "port" in error && error.port === port;
}
