// #666: node selection during a core restart publishes an info hint (not a pending
// failure) and the hint resolves itself once the core reports Running.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { startVite, stubSource, launchBrowser } from "./verify-lib.mjs";

const out = process.env.CB_RESTART_EVIDENCE || "/tmp/cb666-preview";
mkdirSync(out, { recursive: true });
const vite = await startVite();
const browser = await launchBrowser(true);
try {
  const context = await browser.newContext({ viewport: { width: 1120, height: 734 } });
  await context.addInitScript(stubSource());
  await context.route("**/*", (r) => new URL(r.request().url()).origin === new URL(vite.url).origin ? r.continue() : r.abort());
  await context.addInitScript(() => {
    const nodes = [
      { name: "🇯🇵 Tokyo", kind: "VLESS", alive: true, delay_ms: 88 },
      { name: "🇸🇬 Singapore", kind: "VLESS", alive: true, delay_ms: 120 },
    ];
    window.__cb.setCommands({
      proxies_test_snapshot: () => ({ context: { revision: 1, pid: 4242 }, groups: [{ name: "Node selection", kind: "Selector", now: nodes[0].name, nodes }] }),
      node_quality_report: {
        paused: false, epoch: 0, retentionDays: 7, capacity: 8192, storedSamples: 0, storageError: null, pendingSave: false,
        testTargets: ["google-http"], targets: ["google-http"].map((target) => ({ target, total: 0, eligible: 0, successes: 0, timeouts: 0, failures: 0, controllerErrors: 0, successRate: null, p50Ms: null, p95Ms: null, jitterMs: null, samples: [] })),
      },
      select_proxy: () => { throw "core:restarting: Connection refused (os error 61)"; },
    });
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot").waitFor();
  await page.locator(".cb-primary-nav button", { hasText: "节点" }).click();
  const card = page.locator(".cb-node-card").first();
  await card.waitFor();
  await card.click();

  // Silent commands only record payloads in the stub; assert what the service sent.
  await page.waitForFunction(() => (window.__cb.payloadLog.notification_publish ?? []).some((p) => p?.notice?.source?.startsWith("proxy-select")));
  const published = await page.evaluate(() => (window.__cb.payloadLog.notification_publish ?? []).map((p) => p.notice));
  const hint = published.find((n) => n.source.startsWith("proxy-select"));
  assert.equal(hint.kind, "info", "重启窗口内的节点切换只给提示，不产生失败通知");
  assert(hint.description.includes("重启"), "提示说明内核正在重启");
  assert.equal(published.filter((n) => n.kind === "error").length, 0, "没有失败通知");

  // Show the real notice in the production notification centre.
  const showNotice = async (resolved) => {
    await page.evaluate(({ notice, resolved }) => {
      const filled = {
        id: "restart-hint", code: "", title: "", titleKey: "", details: "", operationId: notice.source,
        dedupeKey: notice.source, duration: null, actions: [], progress: null, readAt: null,
        createdAt: Date.now(), updatedAt: Date.now(), count: 1, dismissed: false,
        ...notice,
        resolvedAt: resolved ? Date.now() : null,
      };
      window.__cb.emit("app://notifications", { revision: resolved ? 3 : 2, active: [], history: [filled], navigateTo: null });
    }, { notice: hint, resolved });
    await page.locator(".cb-notification-trigger").click();
    await page.locator("#notification-history-panel").waitFor();
  };

  await showNotice(false);
  assert.equal(await page.locator("#notification-history-panel", { hasText: "没有待处理的问题" }).count(), 1, "待处理列表不新增条目");
  assert.equal(await page.locator("#notification-history-panel", { hasText: "正在重启内核" }).count(), 1, "提示文案可见");
  await page.screenshot({ path: out + "/restart-hint.png", animations: "disabled" });
  await page.keyboard.press("Escape");

  // Core comes back: App resolves the restart hints, backend marks them resolved.
  await page.evaluate(() => window.__cb.emit("core://status", { state: "Running" }));
  await page.waitForFunction(() => (window.__cb.payloadLog.notification_resolve ?? []).some((p) => p?.source?.startsWith("proxy-select")));
  const resolvedSource = await page.evaluate(() => window.__cb.payloadLog.notification_resolve.at(-1).source);
  assert.equal(resolvedSource, hint.source, "内核就绪后自动恢复对应提示");

  await showNotice(true);
  assert.equal(await page.locator("#notification-history-panel", { hasText: "已恢复" }).count(), 1, "提示显示为已恢复");
  await page.screenshot({ path: out + "/restart-hint-recovered.png", animations: "disabled" });

  assert.deepEqual(errors, []);
  console.log("verify-core-restart-hint 通过：重启窗口内节点切换只提示、不进待处理，内核就绪后自动恢复");
} finally {
  await browser.close();
  await vite.close();
}
