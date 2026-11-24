// #655: the download page fetches the latest release in the browser; this scenario
// pins the TTL, retry and fallback states against the real static build.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { extname, resolve } from "node:path";
import { findChromeExecutable } from "./verify-lib.mjs";

const dist = resolve(process.argv[2] || "apps/site/dist");
const out = process.env.CB_DL_EVIDENCE || "/tmp/crossbow-download";
const prefix = process.env.CB_DL_PREFIX || "after";
const before = process.env.CB_SITE_BEFORE === "1";
mkdirSync(out, { recursive: true });

const server = createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
  const f = resolve(dist, "." + path);
  if (!f.startsWith(dist + "/") && f !== dist) { res.writeHead(403); return res.end(); }
  const file = [f, f + ".html", f + "/index.html"].find((p) => existsSync(p) && statSync(p).isFile());
  if (!file) { res.writeHead(404); return res.end("Not found"); }
  res.setHeader("Content-Type", ({ ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp" })[extname(file)] || "application/octet-stream");
  res.end(readFileSync(file));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const origin = `http://127.0.0.1:${server.address().port}`;
const { chromium } = await import("playwright-core");
const browser = await chromium.launch({ executablePath: findChromeExecutable(), headless: true, args: ["--disable-features=OverlayScrollbar"] });

const release = {
  tag_name: "v0.15.3",
  published_at: "2026-11-23T10:00:00Z",
  assets: [
    { name: "Crossbow_0.15.3_aarch64.dmg", browser_download_url: "https://example.test/a.dmg" },
    { name: "Crossbow_0.15.3_x64-setup.exe", browser_download_url: "https://example.test/a.exe" },
    { name: "Crossbow_0.15.3_linux_x86_64.tar.gz", browser_download_url: "https://example.test/a.tar.gz" },
    { name: "SHA256SUMS", browser_download_url: "https://example.test/SHA256SUMS" },
  ],
};
const cached = (minutesAgo) => ({
  savedAt: new Date(Date.now() - minutesAgo * 60000).toISOString(),
  info: {
    version: "v0.15.2",
    dmgUrl: "https://example.test/old.dmg",
    windowsUrl: "https://example.test/old.exe",
    linuxUrl: "https://example.test/old.tar.gz",
    checksumsUrl: "https://example.test/old-SHA256SUMS",
    date: "2026-11-01",
  },
});

async function scenario(name, { api, cachedMinutes, theme }) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  let apiCalls = 0;
  await context.route("**/*", (r) => {
    const url = new URL(r.request().url());
    if (url.origin === origin) return r.continue();
    if (url.host === "api.github.com") {
      apiCalls++;
      return api === "ok"
        ? r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(release) })
        : r.abort();
    }
    return r.abort();
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  if (cachedMinutes !== undefined) {
    await page.addInitScript((payload) => localStorage.setItem("crossbow-release-cache-v1", JSON.stringify(payload)), cached(cachedMinutes));
  }
  await page.goto(origin + "/zh/download");
  await page.waitForTimeout(4200);
  if (theme === "dark") {
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await page.waitForTimeout(120);
  }
  await page.screenshot({ path: `${out}/${prefix}-${name}.png` });
  const state = await page.evaluate(() => ({
    notice: document.querySelector(".download-notice")?.textContent ?? null,
    headline: document.querySelector(".section-heading p")?.textContent ?? null,
    buttons: [...document.querySelectorAll(".download-platform .primary-button")].length,
  }));
  await context.close();
  return { name, apiCalls, errors, ...state };
}

const fresh = await scenario("fresh-cache", { cachedMinutes: 2, api: "fail" });
const stale = await scenario("stale-cache", { cachedMinutes: 60, api: "fail" });
const failed = await scenario("failed", { api: "fail" });
const ready = await scenario("ready", { api: "ok" });
const dark = await scenario("stale-cache-dark", { cachedMinutes: 60, api: "fail", theme: "dark" });
await browser.close();
server.close();

const summary = [fresh, stale, failed, ready, dark];
mkdirSync(out + "/verification", { recursive: true });
if (!before) {
  assert.equal(fresh.apiCalls, 0, "fresh cache is used without hitting the API");
  assert.equal(fresh.notice, null, "fresh cache shows no warning");
  assert(fresh.headline?.includes("v0.15.2"), "fresh cache renders the cached version");
  assert.equal(stale.notice !== null, true, "stale cache failure falls back to a notice");
  assert(stale.headline?.includes("v0.15.2"), "stale cache keeps the cached version");
  assert(stale.apiCalls >= 2, "stale cache tries the API and one retry before failing");
  assert.equal(failed.notice !== null, true, "no cache failure shows the failure notice");
  assert.equal(failed.buttons, 3, "failure state keeps the release-page links usable");
  assert.equal(failed.headline, "版本信息获取失败");
  assert.equal(ready.notice, null, "successful refresh shows no warning");
  assert(ready.headline?.includes("v0.15.3"), "successful refresh shows the latest version");
  // React 18 hydration warnings (#418/#423/#425) predate this change on main; assert nothing else.
  const known = /Minified React error #(418|423|425)/;
  const unexpected = [...new Set([...fresh.errors, ...stale.errors, ...failed.errors, ...ready.errors])].filter((e) => !known.test(e));
  assert.deepEqual(unexpected, [], "no unexpected page errors");
}
console.log(JSON.stringify(summary, null, 2));
console.log("verify-site-download 通过：10 分钟缓存、失败回退与提示样式");
