// Reuse the isolated IPC fixture and choose the initial stage for this preview.
const params = new URLSearchParams(location.search);
const key = `first-connection-preview:${location.search}`;
if (!sessionStorage.getItem(key)) {
  const stage = params.get("stage") ?? "verify";
  const running = ["takeover", "verify", "established"].includes(stage);
  sessionStorage.setItem(key, JSON.stringify({
    configured: stage !== "import",
    installed: stage !== "download",
    core: running ? "Running" : "Stopped",
    proxy: stage === "verify" || stage === "established",
    tun: false,
    port: 7898,
    downloadFails: params.has("downloadFail"),
    authCancelled: false,
    siteFails: params.has("probeFail"),
  }));
  localStorage.setItem("crossbow:first-connection", stage === "established" ? "complete" : "pending");
}
void import("../v01/preview");
