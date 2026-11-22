// #633: production diagnostics, synthetic IPC only; never touches the user's proxy or DNS.
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { startVite, stubSource, launchBrowser, artifactsDir, revision } from "./verify-lib.mjs";

const out = process.env.CB_DIAG_EVIDENCE || artifactsDir();
mkdirSync(out, { recursive: true });
const before = process.env.CB_DIAG_BEFORE === "1";
const vite = await startVite();
const browser = await launchBrowser(true);
const errors = [];
const measurements = [];
const ok = { class: "ok", httpStatus: 204 };
const fault = {
  mode: "sysproxy", startedAtMs: 1795320000000, resolvedAtMs: null,
  recovery: null, superseded: false, legacy: false, profileId: "synthetic", mixedPort: 7897,
  mixedPortOk: true, sysproxyObserved: ["Wi-Fi: 127.0.0.1:7897", "Ethernet: 127.0.0.1:7897"],
  controller: { class: "ok", httpStatus: 200 }, directPath: "DIRECT",
  directCn: { primary: ok, review: null }, directGlobal: { primary: ok, review: null },
  proxyHttps: { primary: { class: "timeout", httpStatus: null }, review: ok },
};
const steps = ["core_running", "controller", "config_renders", "listener", "sysproxy", "proxied_traffic", "dns_direct"];
const report = {
  context: { os: "macos", os_version: "27", engine: "mihomo", core_state: "running", core_version: "1.19.32", proxy: "configured", tun: "off", mixed_port: 7897 },
  rows: steps.map(id => ({ id, ok: id !== "listener", skipped: false,
    detail: id === "listener" ? "127.0.0.1:7897 connect refused — 请检查端口与内核状态" : "", fix: id === "listener" ? "restart_core" : null })),
};
const context = await browser.newContext({ viewport: { width: 1120, height: 734 } });
await context.route("**/*", route => new URL(route.request().url()).origin === new URL(vite.url).origin ? route.continue() : route.abort());
await context.addInitScript(stubSource());
await context.addInitScript(({ fault, report }) => {
  window.__cb.diagFixture = { fail: false, delay: 0, report };
  window.__cb.setCommands({
    sysproxy_fault_evidence: () => ({ latest: fault, previous: { ...fault, startedAtMs: fault.startedAtMs - 86400000, legacy: true } }),
    run_diagnosis: async ({ runId }) => {
      const f = window.__cb.diagFixture;
      window.__cb.emit("diagnosis://progress", { run_id: runId, id: "core_running", result: null });
      if (f.delay) await new Promise(resolve => setTimeout(resolve, f.delay));
      if (f.fail) throw new Error("Synthetic diagnosis unavailable");
      return structuredClone(f.report);
    },
  });
}, { fault, report });
const page = await context.newPage();
page.on("pageerror", error => errors.push(error.message));
const shot = async name => {
  await page.mouse.move(0, 0);
  await page.screenshot({ path: path.join(out, `${name}.png`), animations: "disabled" });
};
const measure = async label => {
  const value = await page.evaluate(() => {
    const root = document.querySelector(".cb-diagnostics-page");
    const head = root.querySelector(".cb-page-head");
    const scroll = root.querySelector(".cb-diag-scroll");
    const action = root.querySelector(".cb-diag-run") || root.querySelector('.cb-empty button');
    const rect = el => { const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height, bottom: r.bottom, right: r.right }; };
    const hit = el => { const r = el.getBoundingClientRect(); const target = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); return target && el.contains(target); };
    const nestedScroll = [...scroll.querySelectorAll("*")].filter(el =>
      ["auto", "scroll"].includes(getComputedStyle(el).overflowY) && el.scrollHeight > el.clientHeight + 1
    ).map(el => el.className);
    const clippedText = [...scroll.querySelectorAll(".cb-diagnostic-step > div, .cb-diag-disclosure-text, .cb-fault-observed, .cb-fault-ifchip")]
      .filter(el => el.getClientRects().length && el.scrollWidth > el.clientWidth + 1).map(el => el.className);
    return { root: rect(root), head: rect(head), scroll: rect(scroll), action: rect(action), actionHit: !!hit(action),
      viewport: { width: innerWidth, height: innerHeight }, overflowX: root.scrollWidth - root.clientWidth,
      documentOverflow: document.documentElement.scrollHeight - innerHeight, nestedScroll, clippedText };
  });
  measurements.push({ label, ...value });
  if (!before) {
    assert(value.actionHit, `${label}: primary action must be hit-testable`);
    assert(value.action.x >= value.root.x - 1 && value.action.right <= value.root.right + 1, `${label}: action must fit horizontally`);
    assert(value.action.bottom <= value.scroll.y + 1, `${label}: primary action stays above scroll content`);
    assert(value.scroll.bottom <= value.root.bottom + 1, `${label}: content must fit its viewport`);
    assert(value.scroll.height > 80, `${label}: usable scroll viewport`);
    assert(value.overflowX <= 1 && value.documentOverflow <= 1, `${label}: no outer page overflow`);
    assert.deepEqual(value.nestedScroll, [], `${label}: one vertical scroll owner`);
    assert.deepEqual(value.clippedText, [], `${label}: long details must wrap instead of being clipped`);
  }
};
const scrollTop = () => page.locator(".cb-diag-scroll").evaluate(el => { el.scrollTop = 0; });

try {
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
  await page.locator(".cb-diagnostics-button").click();
  const root = page.locator(".cb-diagnostics-page");
  await root.locator(".cb-fault-evidence").waitFor();
  await shot("idle-light");
  await root.locator(".cb-fault-evidence-toggle").click();
  await root.locator(".cb-fault-prev summary").click();
  await root.locator(".cb-dnsprobe-check-toggle").click();
  await scrollTop();
  await measure("1120-all-expanded");
  await shot("expanded-light-top");
  if (!before) {
    await root.locator(".cb-fault-evidence-toggle").scrollIntoViewIfNeeded();
    await shot("evidence-expanded-light");
  }
  await page.setViewportSize({ width: 880, height: 560 });
  await measure("880-all-expanded");
  await shot("expanded-small-top");
  if (before) {
    console.log(JSON.stringify({ baseline: true, out, measurements }, null, 2));
  } else {
    // All three independent disclosure states must preserve header and scroll ownership.
    for (const size of [{ width: 880, height: 560 }, { width: 1120, height: 734 }, { width: 1440, height: 960 }]) {
      await page.setViewportSize(size);
      for (const lang of ["zh", "en"]) for (const theme of ["light", "dark"]) {
        await page.evaluate(({ lang, theme }) => {
          window.__cb.emit("ui://lang", lang); window.__cb.emit("ui://theme", theme);
        }, { lang, theme });
        await page.waitForFunction(theme => document.documentElement.classList.contains("dark") === (theme === "dark"), theme);
        for (let mask = 0; mask < 8; mask++) {
          const faultToggle = root.locator(".cb-fault-evidence-toggle");
          const dnsToggle = root.locator(".cb-dnsprobe-check-toggle");
          const setOpen = async (toggle, wanted) => {
            if ((await toggle.getAttribute("aria-expanded") === "true") !== wanted) await toggle.click();
          };
          await setOpen(faultToggle, !!(mask & 1));
          await root.locator(".cb-fault-prev").evaluate((el, open) => { el.open = open; }, !!(mask & 2));
          await setOpen(dnsToggle, !!(mask & 4));
          await measure(`${size.width}x${size.height}-${lang}-${theme}-${mask}`);
        }
      }
    }
    // Keyboard activation and actual form reachability, rather than only DOM visibility.
    const dnsToggle = root.locator(".cb-dnsprobe-check-toggle");
    await dnsToggle.focus();
    await page.keyboard.press("Enter");
    assert.equal(await dnsToggle.getAttribute("aria-expanded"), "false");
    await page.keyboard.press("Space");
    const input = root.locator(".cb-dnsprobe-field input");
    await input.fill("very-long-synthetic-hostname-for-layout.example.com");
    const dnsButton = root.locator('.cb-dnsprobe-form button[type="submit"]');
    await dnsButton.click({ trial: true });
    // Diagnosis remains reachable even with all auxiliary sections expanded and scrolled down.
    await page.setViewportSize({ width: 880, height: 560 });
    await page.evaluate(() => {
      window.__cb.emit("ui://lang", "en"); window.__cb.emit("ui://theme", "dark");
      window.__cb.diagFixture.delay = 500;
    });
    await dnsButton.click({ trial: true });
    await shot("dns-small-dark-en");
    const run = root.locator(".cb-diag-run");
    await run.click();
    assert(await run.isDisabled(), "running prevents duplicate diagnoses");
    assert.equal(await root.locator(".cb-diag-scroll").evaluate(el => el.scrollTop), 0, "starting from auxiliary tools reveals current diagnosis");
    await measure("running-expanded");
    await shot("running-small-dark-en");
    await root.locator(".cb-diag-summary h2", { hasText: "need attention" }).waitFor();
    await scrollTop();
    await shot("result-small-dark-en");
    assert.equal(await input.inputValue(), "very-long-synthetic-hostname-for-layout.example.com", "DNS draft survives main diagnosis state changes");
    assert((await root.locator(".cb-fault-state").first().textContent()).includes("ongoing"), "review success must not rewrite primary failure as recovered");
    await page.evaluate(() => window.__cb.emit("core://status", { state: "Running" }));
    await root.locator(".cb-diag-summary h2", { hasText: "expired" }).waitFor();
    assert.equal(await root.locator(".cb-diagnostic-step button").count(), 0, "stale reports cannot repair");
    await page.evaluate(() => { window.__cb.diagFixture.fail = true; window.__cb.diagFixture.delay = 0; });
    await run.click();
    await root.locator(".cb-diag-summary h2", { hasText: "not completed" }).waitFor();
    await measure("failed-rerun");
    await page.evaluate(() => {
      const f = window.__cb.diagFixture; f.fail = false;
      f.report.rows = f.report.rows.map(r => ({ ...r, ok: true, detail: "", fix: null }));
    });
    await run.click();
    await root.locator(".cb-diag-summary h2", { hasText: "All applicable" }).waitFor();
    // Check continuously changing height with results, no CSS breakpoint jumps or clipped controls.
    for (const height of [560, 588, 625, 680, 720, 800, 960]) {
      await page.setViewportSize({ width: 1120, height });
      await measure(`resize-${height}`);
    }
    await page.evaluate(() => { window.__cb.emit("ui://lang", "zh"); window.__cb.emit("ui://theme", "light"); });
    await page.setViewportSize({ width: 1120, height: 734 });
    await scrollTop();
    await shot("result-light");
    // History read failure and no record must leave the main job usable.
    await page.evaluate(() => {
      window.__cb.setCommands({ sysproxy_fault_evidence: () => { throw new Error("Synthetic evidence unavailable"); } });
      window.__cb.emit("sysproxy://fault-evidence-updated", {});
      window.dispatchEvent(new Event("focus"));
    });
    await root.locator(".cb-fault-evidence").waitFor({ state: "hidden" });
    await measure("history-load-error");
    // Start with neither a report nor evidence: failure still offers a persistent retry.
    await page.reload();
    await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
    await page.evaluate(() => {
      window.__cb.diagFixture.fail = true;
      window.__cb.setCommands({ sysproxy_fault_evidence: () => ({ latest: null, previous: null }) });
    });
    await page.locator(".cb-diagnostics-button").click();
    await root.locator(".cb-diag-run").click();
    await root.locator(".cb-diag-summary h2", { hasText: "检查未完成" }).waitFor();
    await measure("initial-failure-without-history");
    await shot("initial-failure-light");
    await page.evaluate(fault => {
      const f = window.__cb.diagFixture; f.fail = false;
      f.report.rows[3].detail = "VeryLongSyntheticConnectionFailure".repeat(20) + "；请检查内核与本地端口。";
      fault.sysproxyObserved = ["SyntheticInterface".repeat(20) + " → 127.0.0.1:7897"];
      window.__cb.setCommands({ sysproxy_fault_evidence: () => ({ latest: fault, previous: null }) });
      window.dispatchEvent(new Event("focus"));
    }, fault);
    await root.locator(".cb-diag-run").click();
    await root.locator(".cb-diag-summary h2", { hasText: "需要处理" }).waitFor();
    await root.locator(".cb-fault-evidence-toggle").click();
    await measure("long-details");
    await root.locator("#cb-check-listener button").click({ trial: true });
    await shot("long-details-light");
    assert.deepEqual(errors, [], "no production render errors");
    console.log(JSON.stringify({ passed: true, out, checks: measurements.length, revision: revision() }, null, 2));
  }
} catch (error) {
  await shot("failure");
  throw error;
} finally {
  writeFileSync(path.join(out, "layout.json"), JSON.stringify({ before, revision: revision(), measurements, errors }, null, 2));
  await context.close(); await browser.close(); await vite.close();
}
