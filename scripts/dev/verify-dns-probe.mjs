// #603 production DNS-probe card, isolated synthetic IPC. No real DNS/system proxy/TUN changes.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { startVite, stubSource, launchBrowser, revision } from "./verify-lib.mjs";

const out = process.env.CB_DNS_PROBE_EVIDENCE;
if (out) mkdirSync(out, { recursive: true });
const vite = await startVite();
const browser = await launchBrowser(true);
const failures = [];
let shot = 0;

const path = (over = {}) => ({
  state: "ok", ms: 23, rcode: 0,
  answers: ["142.251.150.120", "142.251.151.120"], ttl: 296, note: null, ...over,
});
const probe = (over = {}) => ({
  domain: "www.gstatic.com", record_type: "A", at_ms: Date.now(), engine: "mihomo",
  profile_id:"synthetic-profile", config_revision:7,
  core_running: true, core_pid: 4242, core_version: "1.19.32", dns_mode: "inherit",
  system: path({rcode:null,ttl:null}), kernel: path({ state: "nxdomain", rcode: 3, answers: [], ttl: null, note: "内核返回 RCODE 3" }),
  verdict: "system_only", hosts_override: false,
  unknown_fields: ["upstream", "cache", "kernel_time"], ...over,
});

try {
  const context = await browser.newContext({ viewport: { width: 1120, height: 734 } });
  await context.route("**/*", route => new URL(route.request().url()).origin === new URL(vite.url).origin ? route.continue() : route.abort());
  await context.addInitScript(stubSource());
  const page = await context.newPage();
  page.on("pageerror", error => failures.push(error.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();

  await page.evaluate((base) => {
    const f = window.__cb.dnsProbe = { fail: false, entry: base, delay: 0 };
    window.__cb.setCommands({
      diagnose_dns: async ({ domain, recordType }) => {
        if (f.delay) await new Promise(resolve => setTimeout(resolve, f.delay));
        if (f.fail) throw new Error("仅支持 ASCII 主机名；国际化域名请填写 punycode");
        return { ...structuredClone(f.entry), domain, record_type: recordType, at_ms: Date.now() };
      },
    });
  }, probe());

  await page.locator(".cb-diagnostics-button").click();
  await page.locator(".cb-diagnostics-page").waitFor({ state: "visible" });
  const card = page.locator(".cb-dnsprobe-check");
  await card.waitFor({ state: "visible", timeout: 8000 });
  assert(await card.locator(".cb-dnsprobe-check-body").isHidden(), "DNS 卡片默认折叠，不能挤压检查列表");
  assert((await card.locator(".cb-dnsprobe-check-sum").textContent()).includes("对比系统解析"),
    "折叠态应说明这块能力，而不是空着");
  const capture = async (name) => {
    await card.scrollIntoViewIfNeeded();
    await page.mouse.move(0, 0);
    if (out) await page.screenshot({ path: `${out}/${String(++shot).padStart(2, "0")}-${name}.png`, animations: "disabled" });
  };
  await capture("collapsed");

  await card.locator(".cb-dnsprobe-check-toggle").click();
  await card.locator(".cb-dnsprobe-check-body").waitFor({ state: "visible" });
  await capture("expanded-empty");

  const input = card.getByRole("textbox", { name: "测试域名" });
  const submit = card.getByRole("button", { name: "检查解析" });
  assert(await submit.isDisabled(), "空输入不允许提交");
  await page.evaluate(() => { window.__cb.dnsProbe.fail = true; });
  await input.fill("例子.cn");
  await submit.click();
  await card.locator(".cb-dnsprobe-error").waitFor();
  assert((await card.locator(".cb-dnsprobe-error").textContent()).includes("ASCII"), "应展示后端给出的具体原因");
  await capture("error");

  await page.evaluate(() => { window.__cb.dnsProbe.fail = false; });
  await input.fill("www.gstatic.com");
  await submit.click();
  await card.locator(".cb-dnsprobe-verdict").waitFor();
  assert((await card.locator(".cb-dnsprobe-verdict").textContent()).includes("系统能解析，内核失败"),
    "结论应直接指出哪条路径失败");
  const rows = card.locator(".cb-dnsprobe-path-row");
  assert.equal(await rows.count(), 2, "系统与内核各一行");
  assert((await rows.nth(0).textContent()).includes("成功"), "系统解析应显示成功");
  assert((await rows.nth(1).textContent()).includes("域名不存在"), "内核解析应显示 NXDOMAIN 状态");
  assert((await rows.nth(1).textContent()).includes("RCODE 3"), "后端原因应保留，不吞掉");
  const unknown = (await card.locator(".cb-dnsprobe-unknown").textContent());
  for (const label of ["上游服务器", "缓存命中", "内核内部耗时"]) {
    assert(unknown.includes(label), `未知字段必须显式标注：${label}`);
  }
  assert.equal(await page.evaluate(() => window.__cb.calls.diagnose_dns), 2, "两次提交各发一次 IPC");
  await capture("result-system-only");

  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (text) => { window.__cb.copiedProbe = text; } },
    });
  });
  await card.locator(".cb-dnsprobe-actions button", { hasText: "复制结果" }).click();
  const copied = await page.evaluate(() => window.__cb.copiedProbe);
  for (const must of ["DNS resolution check", "Domain: www.gstatic.com A", "Verdict: system_only", "Unknown fields: upstream, cache, kernel_time"]) {
    assert(copied.includes(must), `摘要应包含：${must}`);
  }
  for (const banned of ["secret", "token", "password"]) {
    assert(!copied.includes(banned), `摘要不应出现敏感内容：${banned}`);
  }

  await page.evaluate((base) => {
    window.__cb.dnsProbe.entry = {
      ...base, verdict: "kernel_unavailable", core_running: false, core_pid: null,
      kernel: { state: "unavailable", ms: 0, rcode: null, answers: [], ttl: null, note: "内核未运行，只记录了系统解析结果" },
    };
  }, probe());
  await input.fill("nas.lan");
  await submit.click();
  await page.waitForFunction(() => document.querySelector(".cb-dnsprobe-verdict")?.textContent.includes("内核未运行"));
  await capture("kernel-unavailable");

  await page.evaluate((base) => {
    window.__cb.dnsProbe.entry = base;
    window.__cb.dnsProbe.delay = 15;
  }, probe());
  for (let i = 0; i < 6; i++) {
    await input.fill(`h${i}.example.com`);
    await submit.click();
    await page.waitForFunction(n => window.__cb.calls.diagnose_dns === n, 4 + i);
  }
  const items = card.locator(".cb-dnsprobe-history-item");
  // 前几条记录已存在，waitFor 会立即返回；必须等最新一条真的渲染出来再断言顺序。
  await page.waitForFunction(() => document.querySelector(".cb-dnsprobe-history-item")?.textContent?.includes("h5.example.com"));
  assert.equal(await items.count(), 5, "历史最多保留 5 条");
  assert((await items.first().textContent()).includes("h5.example.com"), "最新一条排在最前");
  await capture("history");
  await card.locator(".cb-dnsprobe-actions button", { hasText: "清除历史" }).click();
  assert.equal(await items.count(), 0, "清除后不留记录");

  await page.evaluate(() => { window.__cb.emit("ui://theme", "dark"); window.__cb.emit("ui://lang", "en"); });
  await page.setViewportSize({ width: 880, height: 560 });
  await card.getByRole("textbox", { name: "Test domain" }).waitFor();
  await card.locator(".cb-dnsprobe-form button[type=submit]").click();
  await card.locator(".cb-dnsprobe-verdict").waitFor();
  await capture("dark-small-en");
  assert(await card.evaluate(el => el.scrollWidth <= el.clientWidth), "最小窗口下 DNS 卡片不得横向溢出");

  // A single record must be clearable; late results must not resurrect it.
  await card.getByRole("button",{name:"Clear history",exact:true}).click();
  assert.equal(await card.locator(".cb-dnsprobe-result").count(),0);
  await page.evaluate(()=>{window.__cb.dnsProbe.delay=300;});
  await card.locator(".cb-dnsprobe-form button[type=submit]").click();
  await card.getByRole("button",{name:"Cancel",exact:true}).click();
  await page.waitForTimeout(400);
  assert.equal(await card.locator(".cb-dnsprobe-result").count(),0,"cancelled lookup must not reappear");
  await card.locator(".cb-dnsprobe-form button[type=submit]").click();
  await page.evaluate(()=>window.__cb.emit("profile://switched","changed-profile"));
  await page.waitForTimeout(400);
  assert.equal(await card.locator(".cb-dnsprobe-result").count(),0,"old profile result must be discarded");
  await capture("cancelled-stale");
  const unknownCommands = await page.evaluate(() => window.__cb.unknown);
  assert.equal(unknownCommands.length, 0, `出现未注册的 IPC 命令：${JSON.stringify(unknownCommands.slice(0, 3))}`);
} catch (error) {
  failures.push(error.message);
} finally {
  await browser.close();
  await vite.close();
}
if (out) {
  const { writeFileSync } = await import("node:fs");
  writeFileSync(`${out}/result.json`, JSON.stringify({ revision: revision(), shots: shot, failures }, null, 2));
}
if (failures.length) {
  console.error("verify-dns-probe 失败：");
  failures.forEach(failure => console.error(" -", failure));
  process.exit(1);
}
console.log("verify-dns-probe 通过：折叠态、两侧状态、未知字段、历史边界、复制脱敏、深色最小窗口");
