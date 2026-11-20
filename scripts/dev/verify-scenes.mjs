import {
  startVite,
  stubSource,
  launchBrowser,
  sleep,
  artifactsDir,
} from "./verify-lib.mjs";
import { mkdirSync, writeFileSync } from "node:fs";
const dir = process.env.SCENES_EVIDENCE || artifactsDir();
mkdirSync(dir, { recursive: true });
const after = process.env.SCENES_AFTER !== "0";
const vite = await startVite();
const browser = await launchBrowser(true);
const data = {
  rules: { "Home-5G": "direct", "Office Wi-Fi": "proxy" },
  currentSsid: "Home-5G",
  activeScene: ["Home-5G", "direct"],
  supported: true,
  networkState: "available",
};
try {
  const ctx = await browser.newContext({
    viewport: { width: 1120, height: 720 },
  });
  await ctx.addInitScript(stubSource());
  const p = await ctx.newPage();
  const errors = [];
  p.on("pageerror", (e) => errors.push(String(e)));
  await p.goto(vite.url);
  await p.locator(".cb-system-foot").waitFor();
  await p.evaluate(
    (data) => window.__cb.setCommands({ network_scenes_get: data }),
    data,
  );
  await p.locator(".cb-primary-nav button", { hasText: "设置" }).click();
  await p.locator(".cb-scene-rules").scrollIntoViewIfNeeded();
  await sleep(350);
  await p.screenshot({ path: dir + "/light.png" });
  if (after) {
    await p
      .locator(".cb-settings-group")
      .filter({ has: p.locator(".cb-scene-rules") })
      .screenshot({ path: dir + "/detail.png" });
    writeFileSync(
      dir + "/preview.html",
      (await p.content()).replace(/<script[^>]*>[\s\S]*?<\/script>/g, ""),
    );
  }
  if (after) {
    for (const [name, w, h, theme, lang] of [
      ["dark", 1120, 720, "dark", "zh"],
      ["small", 880, 560, "light", "zh"],
      ["english", 1120, 720, "light", "en"],
    ]) {
      await p.setViewportSize({ width: w, height: h });
      await p.evaluate(
        ({ theme, lang }) => {
          window.__cb.emit("ui://theme", theme);
          window.__cb.emit("ui://lang", lang);
        },
        { theme, lang },
      );
      await p.locator(".cb-scene-rules").scrollIntoViewIfNeeded();
      await sleep(250);
      await p.screenshot({ path: dir + "/" + name + ".png" });
    }
    await p.evaluate(() => window.__cb.emit("ui://lang", "zh"));
    await p.setViewportSize({ width: 1120, height: 720 });
    await p.getByRole("button", { name: "添加网络", exact: true }).click();
    await p.getByRole("dialog").waitFor();
    await p.screenshot({ path: dir + "/add.png" });
    await p.keyboard.press("Escape");
    for (const [name, value] of [
      [
        "unavailable",
        {
          ...data,
          currentSsid: null,
          activeScene: null,
          networkState: "unavailable",
        },
      ],
      [
        "unsupported",
        { ...data, supported: false, currentSsid: null, activeScene: null },
      ],
      ["empty", { ...data, rules: {}, activeScene: null }],
    ]) {
      await p.evaluate(
        (v) => window.__cb.setCommands({ network_scenes_get: v }),
        value,
      );
      await p
        .getByRole("button", { name: "刷新网络状态", exact: true })
        .click();
      await sleep(150);
      await p.screenshot({ path: dir + "/" + name + ".png" });
      if (
        name === "unsupported" &&
        !(await p
          .getByRole("button", { name: "添加网络", exact: true })
          .isDisabled())
      )
        throw Error("unsupported platform must disable edits");
    }
    await p.evaluate(() =>
      window.__cb.setCommands({
        network_scenes_get: () => Promise.reject("read failed"),
      }),
    );
    await p.getByRole("button", { name: "刷新网络状态", exact: true }).click();
    await sleep(150);
    await p.screenshot({ path: dir + "/error.png" });
    if (
      !(await p.getByRole("button", { name: "重试", exact: true }).isVisible())
    )
      throw Error("missing retry");
    await p.evaluate((data) => {
      window.__cb.sceneWrites = [];
      window.__cb.sceneData = data;
      window.__cb.setCommands({
        network_scenes_get: () => window.__cb.sceneData,
        network_scene_set: ({ ssid, action }) =>
          new Promise((resolve) => {
            window.__cb.sceneWrites.push({ ssid, action });
            window.__cb.finishWrite = () => {
              window.__cb.sceneData.rules[ssid] = action;
              resolve(null);
            };
          }),
        network_scene_remove: ({ ssid }) => {
          delete window.__cb.sceneData.rules[ssid];
        },
      });
    }, data);
    await p.getByRole("button", { name: "重试", exact: true }).click();
    await sleep(100);
    await p.getByRole("button", { name: "添加网络", exact: true }).click();
    const dialog = p.getByRole("dialog");
    await dialog.getByLabel("Wi-Fi 名称", { exact: true }).fill("Cafe guest");
    await dialog.getByRole("button", { name: "保存规则", exact: true }).click();
    await sleep(50);
    await dialog
      .locator("form")
      .evaluate((form) =>
        form.dispatchEvent(
          new Event("submit", { bubbles: true, cancelable: true }),
        ),
      );
    if ((await p.evaluate(() => window.__cb.sceneWrites.length)) !== 1)
      throw Error("duplicate scene write");
    await p.evaluate(() => window.__cb.finishWrite());
    await sleep(180);
    if (!(await p.getByText("Cafe guest", { exact: true }).isVisible()))
      throw Error("save did not refresh rules");
    const unknown = await p.evaluate(() => window.__cb.unknown);
    if (unknown.length) throw Error("unknown commands " + unknown);
    writeFileSync(
      dir + "/checks.json",
      JSON.stringify(
        {
          viewport: [1120, 720],
          states: [
            "light",
            "dark",
            "small",
            "english",
            "add",
            "unavailable",
            "unsupported",
            "empty",
            "error",
          ],
          errors,
        },
        null,
        2,
      ),
    );
  }
  if (errors.length) throw Error(errors.join("\n"));
  console.log("SceneRules production preview passed: " + dir);
} finally {
  await browser.close();
  await vite.close();
}
