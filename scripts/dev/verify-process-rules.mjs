// Production React UI + isolated IPC fixtures. No real subscription/proxy changes.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { startVite, stubSource, launchBrowser } from "./verify-lib.mjs";
const vite = await startVite();
const browser = await launchBrowser(true);
const out = process.env.CB_PROCESS_EVIDENCE;
if (out) mkdirSync(out, { recursive: true });
try {
  const context = await browser.newContext({
    viewport: { width: 1120, height: 720 },
  });
  await context.route("**/*", (route) =>
    new URL(route.request().url()).origin === new URL(vite.url).origin
      ? route.continue()
      : route.abort(),
  );
  await context.addInitScript(stubSource());
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
  await page.evaluate(() => {
    const rules = [
      { id: "a", process: "Telegram", target: "国际媒体" },
      { id: "b", process: "BankClient", target: "DIRECT" },
    ];
    window.__cb.failSave = true;
    window.__cb.saveRequests = 0;
    window.__cb.setCommands({
      process_rules_get: () => ({
        rules,
        targets: ["DIRECT", "REJECT", "国际媒体", "自动选择"],
      }),
      process_rule_set: (args) => {
        window.__cb.saveRequests++;
        if (window.__cb.failSave) throw Error("mock failure");
        const previous = rules.find((r) => r.id === args.id);
        if (previous) Object.assign(previous, args);
        else rules.push({ ...args, id: "new" });
      },
      process_rule_remove: ({ id }) => {
        rules.splice(
          rules.findIndex((r) => r.id === id),
          1,
        );
      },
    });
  });
  await page.locator(".cb-primary-nav button", { hasText: "配置" }).click();
  await page.getByRole("button", { name: "进程规则", exact: true }).click();
  await page.getByText("BankClient", { exact: true }).waitFor();
  await page.waitForTimeout(300);
  if (out) await page.screenshot({ path: `${out}/light.png` });
  await page.getByRole("button", { name: "编辑", exact: true }).first().click();
  const dialog = page.getByRole("dialog");
  const input = dialog.getByRole("textbox");
  await input.fill("Telegram Desktop");
  await dialog.getByRole("button", { name: "保存", exact: true }).click();
  await page.waitForFunction(() => window.__cb.saveRequests === 1);
  await page.waitForTimeout(50);
  assert(await dialog.isVisible(), "Failed save must keep editor open");
  assert.equal(await input.inputValue(), "Telegram Desktop");
  await page.waitForTimeout(300);
  if (out) await page.screenshot({ path: `${out}/failed-save.png` });
  await page.evaluate(() => (window.__cb.failSave = false));
  await dialog.getByRole("button", { name: "保存", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.getByText("Telegram Desktop", { exact: true }).waitFor();
  await page.waitForFunction(
    () => !document.querySelector(".cb-process-toolbar button")?.disabled,
  );
  await page.evaluate(() => window.__cb.emit("ui://theme", "dark"));
  await page.waitForFunction(() =>
    document.documentElement.classList.contains("dark"),
  );
  await page.setViewportSize({ width: 880, height: 560 });
  await page.waitForTimeout(300);
  if (out) await page.screenshot({ path: `${out}/dark-small.png` });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
    false,
    "Small window must not overflow horizontally",
  );
  await page.evaluate(() => window.__cb.emit("ui://lang", "en"));
  await page.getByRole("button", { name: "Add rule", exact: true }).waitFor();
  await page.getByRole("button", { name: "Edit", exact: true }).first().click();
  await page
    .getByRole("dialog")
    .getByRole("textbox")
    .fill("VeryLongApplicationName".repeat(4));
  await page.waitForTimeout(300);
  if (out) await page.screenshot({ path: `${out}/editor-en-small.png` });
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() => window.__cb.emit("ui://lang", "zh"));
  await page
    .getByRole("button", { name: "移除", exact: true })
    .first()
    .waitFor();
  await page.getByRole("button", { name: "移除", exact: true }).first().click();
  await page
    .getByText("Telegram Desktop", { exact: true })
    .waitFor({ state: "hidden" });
  await page.getByRole("button", { name: "移除", exact: true }).first().click();
  await page.getByText("为应用指定连接方式", { exact: true }).waitFor();
  await page.waitForTimeout(300);
  if (out) await page.screenshot({ path: `${out}/empty.png` });
  await page.locator(".cb-primary-nav button", { hasText: "监测" }).click();
  await page.locator(".cb-connections-page").waitFor();
  await page.evaluate(() =>
    window.__cb.emit("connections://data", {
      rows: [
        {
          id: "preview-row",
          host: "example.invalid",
          rule: "MATCH",
          chains: "DIRECT",
          process: "PreviewProcess",
          up: 0,
          down: 0,
          network: "tcp",
          kind: "HTTP",
          elapsed_ms: 1000,
        },
      ],
      upload_total: 0,
      download_total: 0,
      memory: 0,
    }),
  );
  await page
    .getByRole("button", { name: "为此进程创建分流规则", exact: true })
    .click();
  const preset = page.getByRole("dialog").getByRole("textbox");
  await preset.waitFor();
  assert.equal(
    await preset.inputValue(),
    "PreviewProcess",
    "Connection action must prefill process editor",
  );
  await page.getByRole("button", { name: "取消", exact: true }).click();
  assert.deepEqual(errors, []);
  assert.deepEqual(await page.evaluate(() => window.__cb.unknown), []);
  console.log(
    "Process rules: failed save retains draft; successful retry, removal, empty state, dark/minimum window passed.",
  );
} finally {
  await browser.close();
  await vite.close();
}
