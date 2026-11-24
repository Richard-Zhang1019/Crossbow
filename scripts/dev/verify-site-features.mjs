// #653: real static Astro output for the feature page; no desktop IPC or user data.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { extname, resolve } from "node:path";
import { findChromeExecutable } from "./verify-lib.mjs";

const dist = resolve("apps/site/dist");
const out = process.env.CB_SITE_EVIDENCE || "/tmp/crossbow-site-features";
mkdirSync(out, { recursive: true });
const before = process.env.CB_SITE_BEFORE === "1";
const base = (process.env.SITE_BASE || "").replace(/\/$/, "");

const server = createServer((req, res) => {
  let path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
  if (base) {
    if (!path.startsWith(base + "/")) { res.writeHead(404); return res.end(); }
    path = path.slice(base.length);
  }
  const f = resolve(dist, "." + path);
  if (!f.startsWith(dist + "/") && f !== dist) { res.writeHead(403); return res.end(); }
  const file = [f, f + ".html", f + "/index.html"].find((p) => existsSync(p) && statSync(p).isFile());
  if (!file) { res.writeHead(404); return res.end("Not found"); }
  res.setHeader("Content-Type", ({ ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".woff2": "font/woff2" })[extname(file)] || "application/octet-stream");
  res.end(readFileSync(file));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const origin = `http://127.0.0.1:${server.address().port}`;
// Classic (non-overlay) scrollbars make the gutter visible, matching users who always show them.
const { chromium } = await import("playwright-core");
const browser = await chromium.launch({ executablePath: findChromeExecutable(), headless: true, args: ["--disable-features=OverlayScrollbar"] });
const failures = [];
try {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  await context.route("**/*", (r) => new URL(r.request().url()).origin === origin ? r.continue() : r.abort());
  const p = await context.newPage();
  p.on("pageerror", (e) => failures.push(e.message));

  await p.goto(origin + base + "/zh/features");
  await p.screenshot({ path: out + (before ? "/before-features.png" : "/features-light.png"), fullPage: before });
  if (before) {
    await p.setViewportSize({ width: 390, height: 844 });
    await p.screenshot({ path: out + "/before-mobile.png" });
    await p.setViewportSize({ width: 1440, height: 1000 });
    await p.goto(origin + base + "/zh/guide/dns");
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 450);
    await p.screenshot({ path: out + "/before-guide-scrollbar.png" });
  } else {
    // Same shell as /guide: sidebar, article, on-page directory.
    assert.equal(await p.locator(".docs-shell").count(), 1, "uses docs shell");
    assert(await p.locator(".docs-sidebar .docs-navigation section").count() >= 5, "grouped directory");
    assert(await p.locator(".docs-onpage nav a").count() === 16, "on-page directory lists every section");
    const pageScroll = await p.evaluate(() => { window.scrollTo(0, 400); return { y: window.scrollY, overflow: getComputedStyle(document.body).overflowY }; });
    assert.equal(pageScroll.y, 0, "page itself does not scroll");
    assert.equal(pageScroll.overflow, "hidden", "workspace keeps the viewport fixed");
    const scrollbar = await p.locator(".docs-article").evaluate((e) => ({ width: e.offsetWidth - e.clientWidth, css: getComputedStyle(e, "::-webkit-scrollbar").width }));
    assert(scrollbar.width > 0 && scrollbar.width <= 12, `article scrollbar stays narrow (got ${scrollbar.width}px)`);
    assert.equal(scrollbar.css, "11px", "thin scrollbar is styled explicitly");
    assert(await p.locator(".docs-article").evaluate((e) => e.scrollHeight > e.clientHeight), "article scrolls internally");

    // Directory search filters topics and shows an empty state.
    const total = await p.locator("[data-doc-topic]").count();
    await p.locator("[data-doc-search]").fill("DNS");
    const visible = await p.locator("[data-doc-topic]:visible").count();
    assert(visible > 0 && visible < total, "search narrows the directory");
    await p.locator("[data-doc-search]").fill("没有这个功能xyz");
    assert(await p.locator("[data-doc-empty]").isVisible(), "empty search state");
    await p.locator("[data-doc-search]").fill("");

    // Anchor navigation still targets the same ids and scrolls the article only.
    await p.locator('.docs-navigation a[href="#linux"]').click();
    assert(await p.locator(".docs-article").evaluate((e) => e.scrollTop > 0), "directory anchors scroll the article");
    assert(p.url().endsWith("#linux"), "anchor keeps the deep link");
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 0);

    // On-page directory jumps and highlights the current section.
    await p.locator(".docs-onpage nav a").last().click();
    assert(await p.locator(".docs-article").evaluate((e) => e.scrollTop > 0), "on-page directory scrolls the article");
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 1200);
    await p.waitForTimeout(150);
    assert(await p.locator('.docs-navigation a[aria-current="true"]').count() === 1, "current section is highlighted");
    await p.screenshot({ path: out + "/features-scrolled.png" });
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 0);

    // English route keeps the same structure.
    const en = await context.request.get(origin + base + "/features");
    assert.equal(en.status(), 200);
    const html = await en.text();
    assert(html.includes("docs-shell") && html.includes("On this page") && html.includes('id="support-matrix"'), "English page shares the shell and anchors");

    await p.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await p.waitForTimeout(100);
    await p.screenshot({ path: out + "/features-dark.png" });
    await p.evaluate(() => { document.documentElement.dataset.theme = "light"; });

    // Mobile keeps the directory collapsed and the article readable.
    await p.goto(origin + base + "/zh/guide/dns");
    const guideBar = await p.locator(".docs-article").evaluate((e) => e.offsetWidth - e.clientWidth);
    assert(guideBar <= 12, `guide article scrollbar stays narrow (got ${guideBar}px)`);
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 450);
    await p.screenshot({ path: out + "/guide-scrollbar.png" });

    await p.goto(origin + base + "/zh/features");
    await p.setViewportSize({ width: 390, height: 844 });
    await p.waitForTimeout(200);
    assert.equal(await p.locator(".docs-sidebar").evaluate((e) => e.open), false, "mobile directory starts collapsed");
    await p.screenshot({ path: out + "/features-mobile.png" });
  }
  assert.deepEqual(failures, []);
} catch (error) {
  failures.push(error.message);
} finally {
  await browser.close();
  server.close();
}
if (failures.length) {
  console.error("verify-site-features 失败：");
  failures.forEach((f) => console.error(" -", f));
  process.exit(1);
}
console.log("verify-site-features 通过：docs shell、筛选、锚点、本文目录、移动端折叠");
