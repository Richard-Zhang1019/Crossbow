// #602: Browser regression for the production policy and built assets.
// Models Tauri's inline-script hashes / style nonces; IPC is stubbed.
// This does not replace native tauri:// / ipc:// validation on each platform.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { DESKTOP_DIR, launchBrowser, startVite, stubSource } from "./verify-lib.mjs";

const config = JSON.parse(await readFile(path.join(DESKTOP_DIR, "src-tauri/tauri.conf.json"), "utf8"));
const security = config.app.security;
const dist = path.join(DESKTOP_DIR, "dist");
const html = await readFile(path.join(dist, "index.html"), "utf8");
const nonce = randomBytes(18).toString("base64");
const hashes = [...html.matchAll(/<script\b(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/g)]
  .map((match) => `'sha256-${createHash("sha256").update(match[1].replace(/\r\n/g, "\n")).digest("base64")}'`);
const policy = security.csp
  .replace(/script-src ([^;]*)/, (_, value) => `script-src ${value} ${hashes.join(" ")}`)
  .replace(/style-src ([^;]*)/, (_, value) => `style-src ${value} 'nonce-${nonce}'`);
const document = html.replace(/<style\b/g, `<style nonce="${nonce}"`);
const mime = { ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" };
const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, "http://localhost").pathname;
    if (pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html", "Content-Security-Policy": policy });
      response.end(document);
      return;
    }
    const file = path.resolve(dist, `.${decodeURIComponent(pathname)}`);
    if (!file.startsWith(`${dist}${path.sep}`)) throw new Error("Invalid path");
    response.writeHead(200, { "Content-Type": mime[path.extname(file)] || "application/octet-stream" });
    response.end(await readFile(file));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
let browser;
let vite;
try {
  browser = await launchBrowser(true);
  const page = await browser.newPage();
  await page.addInitScript(stubSource());
  await page.addInitScript(() => {
    window.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      window.__cspViolations.push({ directive: event.effectiveDirective, blocked: event.blockedURI });
    });
    document.addEventListener("DOMContentLoaded", () => {
      window.__bootDelay = getComputedStyle(document.querySelector('.cbt-navitems .cbt-sk')).getPropertyValue("--i").trim();
    });
  });
  // Prevent a bad policy from ever contacting an external host during the probe.
  let escapedRequests = 0;
  await page.route("https://csp-probe.invalid/**", (route) => {
    escapedRequests++;
    return route.abort();
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.waitForFunction(() => document.querySelector("#root")?.children.length > 0);
  const startup = await page.evaluate(() => ({
    violations: window.__cspViolations,
    bootDelay: window.__bootDelay,
  }));
  console.log("Production startup:", JSON.stringify(startup));
  assert.deepEqual(startup.violations, [], "Approved production resources must not violate CSP");
  assert.equal(startup.bootDelay, "1", "Boot inline style attributes must survive Tauri's style nonce");

  await page.evaluate(async () => {
    const inline = document.createElement("script");
    inline.textContent = "window.__unapprovedScriptRan = true";
    document.head.append(inline);
    const external = document.createElement("script");
    external.src = "https://csp-probe.invalid/probe.js";
    document.head.append(external);
    await fetch("https://csp-probe.invalid/report").catch(() => {});
  });
  await page.waitForFunction(() => window.__cspViolations.length >= 3);
  const denied = await page.evaluate(() => ({ violations: window.__cspViolations, ran: !!window.__unapprovedScriptRan }));
  assert.equal(denied.ran, false);
  assert.equal(escapedRequests, 0, "CSP must stop external probes before the network");
  assert(denied.violations.some((item) => item.directive === "script-src-elem" && item.blocked === "inline"));
  assert(denied.violations.some((item) => item.directive === "script-src-elem" && item.blocked.startsWith("https://csp-probe.invalid")));
  assert(denied.violations.some((item) => item.directive === "connect-src"));

  // null deserializes to None in Tauri and falls back to the production CSP.
  assert(security.devCsp, "Development must have an explicit policy, not null fallback");
  assert(security.devCsp.includes("ws://localhost:5173"), "Vite HMR needs its loopback websocket");
  assert(security.devCsp.includes("'unsafe-inline'"), "Vite's React refresh preamble is inline");
  vite = await startVite();
  const devPage = await browser.newPage();
  await devPage.addInitScript(stubSource());
  await devPage.addInitScript(() => {
    window.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => window.__cspViolations.push(event.effectiveDirective));
  });
  await devPage.route(vite.url, async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), "content-security-policy": security.devCsp } });
  });
  await devPage.goto(vite.url);
  await devPage.waitForFunction(() => document.querySelector("#root")?.children.length > 0);
  assert.deepEqual(await devPage.evaluate(() => window.__cspViolations), [], "Vite preamble and styles must load under devCsp");
  console.log("PASS: production and Vite startup, inline styles, rejected scripts/connections, explicit development policy");
} finally {
  await browser?.close();
  await vite?.close();
  await new Promise((resolve) => server.close(resolve));
}
