// #571 production components with fictional IPC data; never touches the user's store.
import { startVite, stubSource, launchBrowser } from "./verify-lib.mjs";
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import path from "node:path";
const out =
  process.env.CB_RULESETS_OUT ??
  "docs/designs/2026-11-20-rule-sets/v01/previews";
mkdirSync(out, { recursive: true });
const vite = await startVite(),
  browser = await launchBrowser(true);
try {
  const context = await browser.newContext({
    viewport: { width: 1120, height: 720 },
  });
  await context.addInitScript(stubSource());
  await context.addInitScript(() => {
    window.__rulesets = [
      {
        id: "ai",
        name: "AI 服务",
        url: "https://rules.example.invalid/ai.yaml",
        behavior: "domain",
        target: "策略组-自动",
        enabled: true,
        interval_secs: 86400,
        updated_at: 1795161600,
        last_error: null,
        count: 24,
      },
      {
        id: "media",
        name: "国际媒体",
        url: "https://rules.example.invalid/media.yaml",
        behavior: "classical",
        target: "DIRECT",
        enabled: true,
        interval_secs: 21600,
        updated_at: 1795158000,
        last_error: "Fixture source offline",
        count: 48,
      },
    ];
    window.__failSave = false;
    window.__edits = [];
    window.__cb.setCommands({
      rule_sets_get: () => ({
        sets: structuredClone(window.__rulesets),
        targets: ["DIRECT", "REJECT", "策略组-自动"],
      }),
      rule_set_save: ({ draft }) => {
        window.__edits.push(["save", draft]);
        if (window.__failSave)
          throw Error("Fixture validation failed; old cache retained");
        const existing = window.__rulesets.find((v) => v.id === draft.id);
        if (existing) Object.assign(existing, draft);
        else
          window.__rulesets.push({
            ...draft,
            id: "new",
            enabled: true,
            updated_at: 1795161600,
            last_error: null,
            count: 12,
          });
      },
      rule_set_change: ({ id, action }) => {
        window.__edits.push([action, id]);
        const index = window.__rulesets.findIndex((v) => v.id === id);
        if (action === "remove") window.__rulesets.splice(index, 1);
        else if (action === "enable" || action === "disable")
          window.__rulesets[index].enabled = action === "enable";
        else {
          const next = action === "up" ? index - 1 : index + 1;
          [window.__rulesets[index], window.__rulesets[next]] = [
            window.__rulesets[next],
            window.__rulesets[index],
          ];
        }
      },
      rule_set_update: ({ id }) => {
        window.__edits.push(["update", id]);
        throw Error("Fixture source offline; previous cache retained");
      },
    });
  });
  const page = await context.newPage();
  await page.goto(vite.url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor();
  await page.locator(".cb-primary-nav button", { hasText: "配置" }).click();
  if (process.argv.includes("--before")) {
    await page.screenshot({ path: path.join(out, "before.png") });
  } else {
    await page.locator(".cb-subnav button", { hasText: "规则集" }).click();
    const rows = page.locator(".cb-rulesets-row");
    await rows.first().waitFor();
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(out, "light.png") });
    const toggle = page.getByRole("switch", { name: "启用 AI 服务" });
    await toggle.click();
    await page.waitForFunction(() => window.__rulesets[0].enabled === false);
    assert.equal(await toggle.getAttribute("aria-checked"), "false");
    await rows
      .nth(1)
      .getByRole("button", { name: /^操作 / })
      .click();
    await page.getByRole("button", { name: "上移", exact: true }).click();
    await page.waitForFunction(() => window.__rulesets[0].id === "media");
    await rows.first().getByText("国际媒体", { exact: true }).waitFor();
    await rows
      .first()
      .getByRole("button", { name: /^操作 / })
      .click();
    await page.getByRole("button", { name: "编辑", exact: true }).click();
    await page.getByLabel("名称", { exact: true }).fill("国际媒体 · 已编辑");
    await page.evaluate(() => {
      window.__failSave = true;
    });
    await page.getByRole("button", { name: "保存", exact: true }).click();
    await page.waitForFunction(() =>
      window.__edits.some((v) => v[0] === "save"),
    );
    await page.waitForFunction(
      () =>
        document.querySelector('[role="dialog"] button[type="submit"]')
          ?.disabled === false,
    );
    assert(await page.locator(".cb-dialog").isVisible());
    assert.equal(
      await page.getByLabel("名称", { exact: true }).inputValue(),
      "国际媒体 · 已编辑",
    );
    assert.equal(
      await page.evaluate(() => window.__rulesets[0].name),
      "国际媒体",
    );
    await page.evaluate(() => {
      window.__failSave = false;
    });
    await page.getByRole("button", { name: "保存", exact: true }).click();
    await page.locator(".cb-dialog").waitFor({ state: "hidden" });
    await page.getByRole("button", { name: "添加规则集", exact: true }).click();
    await page.getByLabel("名称", { exact: true }).fill("开发工具");
    await page
      .getByLabel("规则集链接", { exact: true })
      .fill("https://rules.example.invalid/dev.yaml");
    await page.getByRole("button", { name: "保存", exact: true }).click();
    await page.locator(".cb-dialog").waitFor({ state: "hidden" });
    assert.equal(await rows.count(), 3);
    await page.evaluate(() => window.__cb.emit("ui://theme", "dark"));
    await page.setViewportSize({ width: 880, height: 560 });
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(out, "dark-small.png") });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
    );
    await page.getByRole("button", { name: "添加规则集", exact: true }).click();
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(out, "form-small.png") });
    const box = await page.locator(".cb-dialog").boundingBox();
    assert(box && box.y >= 0 && box.y + box.height <= 560);
    const submit = await page
      .getByRole("button", { name: "保存", exact: true })
      .boundingBox();
    assert(
      submit && submit.y + submit.height <= 536,
      "submit must remain visible in the minimum window",
    );
    await page.keyboard.press("Escape");
    await page.evaluate(() => window.__cb.emit("ui://lang", "en"));
    await page
      .getByRole("button", { name: "Add rule set", exact: true })
      .waitFor();
    await page.waitForTimeout(200);
    await page.screenshot({ path: path.join(out, "english-small.png") });
    await page.evaluate(() => {
      window.__rulesets = [];
      window.__cb.emit("rulesets://updated", null);
    });
    await page
      .getByText("Route your services to a policy", { exact: true })
      .waitFor();
    await page.screenshot({ path: path.join(out, "empty.png") });
    console.log(
      "PASS: load, toggle, reorder, failed save retains form/old data, retry, add, dark/small/en, empty, no overflow",
    );
  }
} finally {
  await browser.close();
  await vite.close();
}
