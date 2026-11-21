// Production DNS editor, isolated synthetic IPC. No real subscriptions/proxy/TUN changes.
import assert from "node:assert/strict";
import { mkdirSync, writeFileSync } from "node:fs";
import { startVite, stubSource, launchBrowser, revision } from "./verify-lib.mjs";
const out = process.env.CB_DNS_EVIDENCE;
if (out) mkdirSync(out, { recursive: true });
const vite = await startVite();
const browser = await launchBrowser(true);
const failures = [];
try {
  const context = await browser.newContext({ viewport: { width: 1120, height: 734 } });
  await context.route("**/*", route => new URL(route.request().url()).origin === new URL(vite.url).origin ? route.continue() : route.abort());
  await context.addInitScript(stubSource());
  const page = await context.newPage();
  page.on("pageerror", error => failures.push(error.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
  await page.evaluate(() => {
    const f = window.__cb.dnsFixture = {
      failRead: !Boolean(window.localStorage.getItem("dns-before")), failSave: true, engine: "mihomo", delay: 0,
      stored: { bootstrap_nameservers: [], direct_nameservers: ["udp://192.0.2.53:5353"], mode: "custom", proxy_nameservers: ["https://resolver.test/dns-query"] },
    };
    window.__cb.setCommands({
      set_engine: ({ engine }) => { f.engine = engine; },
      get_dns_settings: () => { if (f.failRead) throw new Error("Simulated DNS read failure"); return structuredClone(f.stored); },
      get_engine_config: () => ({ engine: f.engine, mixed_port: 7897, allow_lan: false, flag_emoji: true, core_installed: true }),
      set_dns_settings: async ({ settings }) => {
        await new Promise(resolve => setTimeout(resolve, f.delay));
        if (f.failSave) throw new Error("Simulated DNS transaction rollback");
        f.stored = structuredClone(settings);
      },
    });
  });
  if (process.env.CB_DNS_SCREENSHOTS_ONLY) await page.evaluate(() => {
    window.__cb.dnsFixture.failRead = false;
    window.__cb.dnsFixture.stored.proxy_nameservers = ["https://changed.test:8443/dns-query"];
  });
  await page.locator(".cb-primary-nav button", { hasText: "设置" }).click();
  const card = page.locator(".cb-dns-card");
  const capture = async name => {
    await card.scrollIntoViewIfNeeded();
    await page.mouse.move(0, 0);
    if (out) await page.screenshot({ path: `${out}/${name}.png`, animations: "disabled" });
    assert(await card.evaluate(el => el.scrollWidth <= el.clientWidth), "DNS editor must fit the settings width");
  };
  if (!process.env.CB_DNS_SCREENSHOTS_ONLY) {
    await card.getByRole("button", { name: "重试", exact: true }).waitFor();
    assert(await card.getByRole("radio", { name: "自定义", exact: true }).isDisabled());
    await capture("load-failed");
    await page.evaluate(() => { window.__cb.dnsFixture.failRead = false; });
    await card.getByRole("button", { name: "重试", exact: true }).click();
  }
  const proxy = card.getByRole("textbox", { name: "代理业务上游", exact: true });
  await proxy.waitFor();
  if (!process.env.CB_DNS_SCREENSHOTS_ONLY) {
    const save = card.getByRole("button", { name: "保存并应用", exact: true });
    assert(await save.isDisabled(), "Unchanged settings should not be resubmitted");
    await proxy.fill("https://changed.test:8443/dns-query");
    await save.click();
    await page.waitForFunction(() => window.__cb.calls.set_dns_settings === 1 && !document.querySelector(".cb-dns-fields").disabled);
    assert.equal(await proxy.inputValue(), "https://changed.test:8443/dns-query", "Failed save must keep the editable draft");
    assert.equal(await page.evaluate(() => window.__cb.dnsFixture.stored.proxy_nameservers[0]), "https://resolver.test/dns-query");
    assert.equal(await page.evaluate(() => window.__cb.calls.apply_overrides ?? 0), 0, "DNS must be one transaction, never two IPC writes");
    await capture("apply-failed");
    await page.evaluate(() => { window.__cb.dnsFixture.failSave = false; window.__cb.dnsFixture.delay = 350; });
    await save.click();
    assert(await proxy.isDisabled(), "Pending apply must freeze its draft");
    assert(await card.getByRole("radio", { name: "沿用订阅", exact: true }).isDisabled());
    await page.waitForFunction(() => window.__cb.calls.set_dns_settings === 2 && !document.querySelector(".cb-dns-fields").disabled);
    assert(await save.isDisabled());
  }
  await capture("light-custom");
  await page.evaluate(() => { window.__cb.emit("ui://theme", "dark"); window.__cb.emit("ui://lang", "en"); });
  await page.setViewportSize({ width: 880, height: 560 });
  await card.getByRole("textbox", { name: "Proxy upstream", exact: true }).waitFor();
  await capture("dark-small");
  if (out) {
    await card.getByRole("button", { name: "Save & apply", exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: `${out}/dark-small-footer.png`, animations: "disabled" });
  }
  if (!process.env.CB_DNS_SCREENSHOTS_ONLY) {
    await page.getByRole("combobox", { name: "Core engine", exact: true }).click();
    await page.getByRole("option", { name: "sing-box", exact: true }).click();
    await page.getByRole("button", { name: "Enable", exact: true }).click();
    const bootstrap = card.getByRole("textbox", { name: "Bootstrap upstream", exact: true });
    await page.waitForFunction(() => document.querySelectorAll(".cb-dns-card input:disabled").length === 1);
    assert(await bootstrap.isDisabled());
    await capture("dark-singbox-disabled");
    await card.getByRole("radio", { name: "Inherit", exact: true }).click();
    await card.getByRole("button", { name: "Save & apply", exact: true }).click();
    await page.waitForFunction(() => window.__cb.dnsFixture.stored.mode === "inherit");
    assert.equal(await card.getByRole("textbox").count(), 0);
  }
  assert.deepEqual(await page.evaluate(() => window.__cb.unknown), []);
  assert.deepEqual(failures, []);
  if (out) writeFileSync(`${out}/evidence.json`, JSON.stringify({ revision: revision(), platform: process.platform, browser: browser.version(), source: "Production Settings/DnsSettings with synthetic IPC; no native networking", views: ["1120x734 light zh", "880x560 dark en"], failures }, null, 2));
  console.log(process.env.CB_DNS_SCREENSHOTS_ONLY ? "DNS production UI: screenshot-only capture" : "DNS production UI: read failure/retry, atomic save failure/draft, pending lock, success, engine change, inherit, min-window pass");
} finally { await browser.close(); await vite.close(); }
