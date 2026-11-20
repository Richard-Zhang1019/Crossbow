// Production Settings UI exercised with isolated IPC fixtures.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { startVite, stubSource, launchBrowser } from "./verify-lib.mjs";

const vite = await startVite();
const browser = await launchBrowser(true);
const out = process.env.CB_LAN_EVIDENCE;
if (out) mkdirSync(out, { recursive: true });

try {
  const context = await browser.newContext({
    viewport: { width: 1120, height: 734 },
  });
  await context.route("**/*", (route) =>
    new URL(route.request().url()).origin === new URL(vite.url).origin
      ? route.continue()
      : route.abort(),
  );
  await context.addInitScript(stubSource());
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
  await page.evaluate(() => {
    const fixture = {
      allow_lan: false,
      mixed_port: 7897,
      failToggle: false,
      failRead: false,
      addresses: [
        { interface: "en0", address: "192.168.1.24" },
        { interface: "en5", address: "10.0.0.20" },
      ],
    };
    window.__cb.lanFixture = fixture;
    window.__cb.setCommands({
      get_engine_config: () => ({
        engine: "mihomo",
        mixed_port: fixture.mixed_port,
        allow_lan: fixture.allow_lan,
        flag_emoji: true,
        unified_delay: true,
        core_installed: true,
      }),
      set_allow_lan: ({ enabled }) => {
        if (fixture.failToggle) throw new Error("Simulated listener restart failure");
        fixture.allow_lan = enabled;
      },
      set_mixed_port: ({ port }) => { fixture.mixed_port = port; },
      lan_addresses: () => {
        if (fixture.failRead) throw new Error("Simulated interface read failure");
        return fixture.addresses;
      },
    });
    Object.defineProperty(navigator, "clipboard", {
      value: {
        async writeText(text) {
          window.__cb.clipboard = text;
        },
      },
    });
  });

  await page.locator(".cb-primary-nav button", { hasText: "设置" }).click();
  const toggle = page.getByRole("switch", { name: "允许局域网连接" });
  const panel = page.locator(".cb-lan-access");
  await toggle.scrollIntoViewIfNeeded();
  await page.getByText("可访问此端口的设备均可使用代理。", { exact: false }).waitFor();
  assert.equal(await panel.count(), 0);
  assert.equal(await page.evaluate(() => window.__cb.calls.lan_addresses ?? 0), 0);
  await toggle.click();
  const firstAddress = panel.getByText("192.168.1.24:7897", { exact: true });
  await firstAddress.waitFor();
  await panel.getByText("10.0.0.20:7897", { exact: true }).waitFor();
  await panel.scrollIntoViewIfNeeded();
  await page.mouse.move(0, 0);
  if (out) await page.screenshot({ path: `${out}/light.png`, animations: "disabled" });

  await panel.getByRole("button", { name: "复制 192.168.1.24:7897" }).click();
  await panel.getByRole("button", { name: "已复制" }).waitFor();
  assert.equal(await page.evaluate(() => window.__cb.clipboard), "192.168.1.24:7897");

  await page.getByRole("button", { name: "修改 混合监听端口" }).click();
  const port = page.getByRole("textbox", { name: "混合监听端口" });
  await port.fill("7898");
  await port.press("Enter");
  await panel.getByText("192.168.1.24:7898", { exact: true }).waitFor();

  const refresh = panel.getByRole("button", { name: "刷新地址" });
  await page.evaluate(() => { window.__cb.lanFixture.failRead = true; });
  await refresh.click();
  await panel.getByRole("status").getByText("无法读取本机网络地址。请刷新重试。").waitFor();
  assert.equal(await panel.locator("li").count(), 0);
  await page.evaluate(() => {
    window.__cb.lanFixture.failRead = false;
    window.__cb.lanFixture.addresses = [];
  });
  await refresh.click();
  await panel.getByRole("status").getByText("暂无可用的 IPv4 地址。连接局域网后刷新。").waitFor();
  await page.evaluate(() => {
    window.__cb.lanFixture.addresses = [
      { interface: "Ethernet 2 / Dock", address: "192.168.50.24" },
      { interface: "Wi-Fi", address: "10.0.0.20" },
    ];
    window.__cb.emit("ui://theme", "dark");
    window.__cb.emit("ui://lang", "en");
  });
  await page.setViewportSize({ width: 880, height: 560 });
  await panel.getByRole("button", { name: "Refresh addresses" }).click();
  await panel.getByText("192.168.50.24:7898", { exact: true }).waitFor();
  await page.waitForFunction(() => document.documentElement.classList.contains("dark"));
  await panel.scrollIntoViewIfNeeded();
  await page.mouse.move(0, 0);
  if (out) await page.screenshot({ path: `${out}/dark-small.png`, animations: "disabled" });
  assert(await panel.evaluate((element) => element.scrollWidth <= element.clientWidth),
    "LAN settings should fit at the minimum window width");

  const englishToggle = page.getByRole("switch", { name: "Allow LAN" });
  await page.evaluate(() => { window.__cb.lanFixture.failToggle = true; });
  await englishToggle.click();
  await page.waitForFunction(() =>
    window.__cb.calls.set_allow_lan === 2
      && !document.querySelector(".cb-settings-fields").disabled,
  );
  assert.equal(await englishToggle.getAttribute("aria-checked"), "true");
  assert(await panel.isVisible(), "Failed disable should keep the confirmed setting");

  await page.evaluate(() => { window.__cb.lanFixture.failToggle = false; });
  await englishToggle.click();
  await panel.waitFor({ state: "detached" });
  assert.equal(await englishToggle.getAttribute("aria-checked"), "false");
  assert.deepEqual(errors, []);
  assert.deepEqual(await page.evaluate(() => window.__cb.unknown), []);
  console.log("LAN settings: toggle, failed disable, multi-interface addresses, clipboard, empty state, retry and small window passed.");
} finally {
  await browser.close();
  await vite.close();
}
