// #632: a successful profile import must not offer the misleading "nodes" action.
// Production React + isolated IPC; no real subscription or core is touched.
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { startVite, stubSource, launchBrowser, revision } from "./verify-lib.mjs";

const out = process.env.CB_IMPORT_EVIDENCE;
if (out) mkdirSync(out, { recursive: true });
const vite = await startVite();
const browser = await launchBrowser(true);
const failures = [];
try {
  const context = await browser.newContext({ viewport: { width: 1120, height: 734 } });
  await context.addInitScript(stubSource());
  await context.route("**/*", route => new URL(route.request().url()).origin === new URL(vite.url).origin ? route.continue() : route.abort());
  const page = await context.newPage();
  page.on("pageerror", error => failures.push(error.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();

  await page.evaluate(() => {
    const fixture = window.__cb.importFixture = { active: "p-a", failRead: false, notices: [] };
    window.__cb.setCommands({
      active_profile_id: () => {
        if (fixture.failRead) throw new Error("synthetic active-profile read failure");
        return fixture.active;
      },
      import_profile_url: ({ url }) => ({
        id: "p-imported", name: "Imported", kind: "remote", url,
        update_interval_min: 0, override_ids: [], last_updated: null, traffic: null, last_update: null,
      }),
      notification_publish: (notice) => { fixture.notices.push(notice); return null; },
    });
  });

  const waitFor = async (label, probe, timeout = 15000, arg = undefined) => {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
      if (await page.evaluate(probe, arg)) return;
      await page.waitForTimeout(100);
    }
    throw new Error(`等待超时：${label}`);
  };
  const importOnce = async (link) => {
    const start = await page.evaluate(() => (window.__cb.payloadLog.notification_publish ?? []).length);
    await page.locator(".cb-primary-nav button", { hasText: "配置" }).click();
    await page.getByRole("button", { name: "添加订阅", exact: true }).first().click();
    const dialog = page.locator(".cb-dialog");
    await dialog.waitFor();
    await dialog.locator("#profile-url").fill(link);
    const submit = dialog.getByRole("button", { name: "＋ 导入", exact: true });
    // The button unlocks only after the local compatibility preflight finishes.
    await waitFor("导入按钮可用", () => {
      const target = [...document.querySelectorAll(".cb-dialog button")]
        .find(button => (button.textContent ?? "").includes("导入"));
      return !!target && !target.disabled;
    });
    await submit.click();
    // notification_publish 是静默命令：stub 保留最近 50 次参数，核对真实发布的动作。
    // 导入后的 refresh 也可能发布一条读取错误，所以只看本次导入之后的成功通知。
    await waitFor("成功通知发布", (start) => (window.__cb.payloadLog.notification_publish ?? [])
      .slice(start).some(entry => entry?.notice?.kind === "success"), undefined, start);
    return page.evaluate((start) => (window.__cb.payloadLog.notification_publish ?? [])
      .slice(start).filter(entry => entry?.notice?.kind === "success").at(-1)?.notice, start);
  };

  // 1) Active subscription A + import B: the success notice must not point at A's nodes.
  const imported = await importOnce("vless://11111111-2222-3333-4444-555555555555@example.invalid:443?type=tcp#B");
  assert.deepEqual(imported.actions ?? [], [], "导入成功通知不应带任何动作");
  assert(!(imported.actions ?? []).includes("nodes"), "不得再把通知指向节点页");
  assert.equal(await page.evaluate(() => window.__cb.calls.set_active_profile ?? 0), 0, "导入不得隐式激活新订阅");
  assert(await page.locator(".cb-profiles-page").isVisible(), "导入后应停留在配置页");
  assert.equal(await page.locator(".cb-proxies-page").count(), 0, "通知动作不得自动跳转节点页");

  // 2) activeProfileId read failure: no action with the wrong context either.
  await page.evaluate(() => { window.__cb.importFixture.failRead = true; });
  const afterFailure = await importOnce("https://example.invalid/sub-b");
  assert.deepEqual(afterFailure.actions ?? [], [], "读取失败时也不应生成跳转动作");
  await page.evaluate(() => { window.__cb.importFixture.failRead = false; });

  // 3) First import (no active profile): setup guidance is still offered.
  await page.evaluate(() => { window.__cb.importFixture.active = null; });
  const firstImport = await importOnce("https://example.invalid/first");
  assert((firstImport.actions ?? []).includes("continueSetup"), "首次导入仍应保留设置引导");

  if (out) {
    // Show the real published notice in the production notification centre.
    await page.evaluate((notice) => {
      const filled = {
        id: "notice-632", code: "import.done", kind: "success", title: "", titleKey: "profiles.title",
        description: notice.description ?? "", details: "", source: "profile-import-0",
        operationId: "profile-import-0", dedupeKey: "", duration: null, progress: null,
        actions: notice.actions ?? [], createdAt: Date.now(), updatedAt: Date.now(),
        readAt: null, resolvedAt: null, count: 1, dismissed: false,
      };
      window.__cb.emit("app://notifications", { revision: 99, active: [], history: [filled], navigateTo: null });
    }, imported);
    await page.locator(".cb-notification-trigger").click();
    await page.locator("#notification-history-panel").waitFor();
    await page.screenshot({ path: `${out}/import-notice-no-node-action.png`, animations: "disabled" });
    assert.equal(await page.locator(".cb-notification-history-panel button").filter({ hasText: "选择节点" }).count(), 0);
  }
  if (out) writeFileSync(`${out}/result.json`, JSON.stringify({ revision: revision(), published: imported.actions ?? [], failures }, null, 2));
} catch (error) {
  failures.push(error.message);
} finally {
  await browser.close();
  await vite.close();
}
if (failures.length) {
  console.error("verify-import-notification 失败：");
  failures.forEach(failure => console.error(" -", failure));
  process.exit(1);
}
console.log("verify-import-notification 通过：普通/读取失败/首次导入三种路径的通知动作正确，未自动激活或跳转");
