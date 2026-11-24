// #653: real static Astro routes for the feature documentation; no desktop IPC or user data.
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
  await p.screenshot({ path: out + (before ? "/before-features.png" : "/features-overview-light.png"), fullPage: before });
  if (before) {
    await p.setViewportSize({ width: 1440, height: 1000 });
    await p.goto(origin + base + "/zh/guide/dns");
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 450);
    await p.screenshot({ path: out + "/before-guide-scrollbar.png" });
  } else {
    // The overview is a documentation index, one link per section route.
    const links = await p.locator(".docs-navigation a").evaluateAll((as) => as.map((a) => a.getAttribute("href")));
    assert.equal(links.length, 16, "directory lists every section");
    assert(links.every((h) => h && h.startsWith(base + "/zh/features/")), "directory links are real routes");
    assert.equal(await p.evaluate(() => document.documentElement.scrollHeight <= innerHeight || getComputedStyle(document.body).overflowY === "hidden"), true, "workspace does not grow the page");
    const articleWidth = await p.locator(".docs-article").evaluate((e) => e.clientWidth);
    assert(articleWidth >= 800, `content column uses the available width (got ${articleWidth}px)`);
    assert.equal(await p.locator(".docs-start .btn-primary").count(), 0, "overview entry is a quiet link, not a primary button");
    const geometry = await p.evaluate(() => {
      const article = document.querySelector(".docs-article").getBoundingClientRect();
      const onpage = document.querySelector(".docs-onpage").getBoundingClientRect();
      const brand = document.querySelector(".nav-shell .brand").getBoundingClientRect();
      const sidebar = document.querySelector(".docs-sidebar").getBoundingClientRect();
      return { gap: Math.round(onpage.left - article.right), brandLeft: Math.round(brand.left), sidebarLeft: Math.round(sidebar.left) };
    });
    assert(geometry.gap >= 36 && geometry.gap <= 44, `on-page directory sits one column gap from the prose (got ${geometry.gap}px)`);
    assert(Math.abs(geometry.brandLeft - geometry.sidebarLeft) <= 1, "header brand aligns with the documentation column");

    // Filter narrows the directory and shows an empty state.
    await p.locator("[data-doc-search]").fill("DNS");
    const visible = await p.locator("[data-doc-topic]:visible").count();
    assert(visible > 0 && visible < 16, "search narrows the directory");
    await p.locator("[data-doc-search]").fill("没有这个功能xyz");
    assert(await p.locator("[data-doc-empty]").isVisible(), "empty search state");
    await p.locator("[data-doc-search]").fill("");

    // Each section is its own route with its own title and current-directory marker.
    await p.locator('.docs-navigation a[href$="/features/nodes"]').click();
    await p.waitForURL("**/zh/features/nodes");
    assert.equal(await p.locator("h1").innerText(), "节点选择与测速");
    assert.equal(await p.locator('.docs-navigation [aria-current="page"]').count(), 1, "current directory item is marked");
    assert.equal(await p.locator(".docs-onpage nav a").count(), 1, "section page lists its in-page heading");
    const scrollbar = await p.locator(".docs-article").evaluate((e) => ({ width: e.offsetWidth - e.clientWidth, css: getComputedStyle(e, "::-webkit-scrollbar").width }));
    assert(scrollbar.width > 0 && scrollbar.width <= 12, `article scrollbar stays narrow (got ${scrollbar.width}px)`);
    assert.equal(scrollbar.css, "11px", "thin scrollbar is styled explicitly");
    await p.screenshot({ path: out + "/features-section-light.png" });

    // Pagination keeps the documentation flow.
    await p.locator(".docs-pagination a").last().click();
    await p.waitForURL("**/zh/features/subscription");
    await p.goBack();
    await p.waitForURL("**/zh/features/nodes");

    // English routes share the shell.
    await p.goto(origin + base + "/features/support-matrix");
    assert.equal(await p.locator("h1").innerText(), "Platform support matrix");
    assert.equal(await p.locator(".docs-navigation [aria-current=page]").count(), 1);
    assert.equal(await p.locator("table.docs-matrix").count(), 1, "matrix renders on its own route");

    // Header, directory search and article gutter keep their tuned geometry (#653 review).
    await p.goto(origin + base + "/zh/guide/dns");
    const header = await p.locator(".nav-shell").evaluate((e) => e.getBoundingClientRect().height);
    assert.equal(Math.round(header), 68, "header uses the compact height");
    const articleMetrics = await p.locator(".docs-article").evaluate((e) => ({ pad: parseFloat(getComputedStyle(e).paddingRight), gutter: e.offsetWidth - e.clientWidth }));
    assert(articleMetrics.pad >= 36, `article keeps space before the scrollbar (got ${articleMetrics.pad}px)`);
    assert(articleMetrics.gutter > 0 && articleMetrics.gutter <= 12, "gutter stays thin");
    const searchBox = await p.locator(".docs-search").evaluate((e) => e.getBoundingClientRect().height);
    assert(searchBox <= 30, `search field stays compact (got ${searchBox}px)`);
    const borderIdle = await p.locator(".docs-search").evaluate((e) => getComputedStyle(e).borderTopColor);
    await p.locator("[data-doc-search]").focus();
    await p.waitForTimeout(250);
    const focusStyle = await p.locator(".docs-search").evaluate((e) => { const s = getComputedStyle(e); return { outline: s.outlineStyle, border: s.borderTopColor, inset: s.boxShadow }; });
    assert.equal(focusStyle.outline, "none", "focus ring stays inside the field (no clipped outline)");
    assert(focusStyle.inset.includes("inset"), "focus ring is an inset shadow");
    assert.notEqual(focusStyle.border, borderIdle, "focus switches the border to the accent color");
    const ring = await p.evaluate(() => {
      const s = document.querySelector(".docs-search").getBoundingClientRect();
      const pane = document.querySelector(".docs-sidebar-body").getBoundingClientRect();
      return { insetLeft: s.left - pane.left };
    });
    assert(ring.insetLeft >= 0, "focus ring is not clipped by the directory pane");
    await p.locator(".docs-article").evaluate((e) => e.scrollTop = 0);
    await p.screenshot({ path: out + "/guide-header-search.png" });

    // Legacy section hashes still land on the matching route.
    await p.goto(origin + base + "/zh/features#diagnostics");
    await p.waitForURL("**/zh/features/diagnostics");

    // All 32 routes exist, have a single title and no broken internal links.
    let pages = 0;
    const broken = [];
    for (const lang of ["zh", "en"]) {
      for (const link of links) {
        const path = lang === "zh" ? link : link.replace("/zh/features/", "/features/");
        const response = await context.request.get(origin + path);
        assert.equal(response.status(), 200, path);
        const html = await response.text();
        assert.equal((html.match(/<h1[ >]/g) || []).length, 1, path + " has one title");
        assert(html.includes("docs-shell") && html.includes('aria-current="page"'), path + " shares the docs shell");
        assert(html.includes('rel="canonical"') && html.includes('hreflang="zh-CN"'), path + " has language metadata");
        const checks = [...html.matchAll(/href="([^"]+)"/g)].map((m) => m[1].replaceAll("&amp;", "&")).filter((h) => h.startsWith(base + "/") && !h.startsWith("//"));
        for (const h of new Set(checks)) {
          const target = await context.request.get(new URL(h, origin).href);
          if (target.status() !== 200) broken.push(new URL(h, origin).pathname);
        }
        pages++;
      }
    }
    assert.equal(pages, 32, "every section has a route in both languages");
    assert.deepEqual([...new Set(broken)], [], "internal links");

    // Dark theme and mobile keep the workspace usable.
    await p.goto(origin + base + "/zh/features/nodes");
    await p.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await p.waitForTimeout(100);
    await p.screenshot({ path: out + "/features-section-dark.png" });
    await p.goto(origin + base + "/zh/features");
    await p.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await p.waitForTimeout(100);
    await p.screenshot({ path: out + "/features-overview-dark.png" });
    await p.evaluate(() => { document.documentElement.dataset.theme = "light"; });
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
console.log("verify-site-features 通过：16 节独立路由、目录筛选、当前项、分页、旧锚点迁移、明暗与移动端");
