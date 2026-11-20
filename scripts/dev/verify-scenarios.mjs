#!/usr/bin/env node
// #511：可复跑桌面验证入口——生产组件 + IPC 桩，固化关键回归场景。
//
// 用法（仓库根目录）：
//   node scripts/dev/verify-scenarios.mjs                # 全部场景（headless）
//   node scripts/dev/verify-scenarios.mjs --scenario 2   # 只跑某场景
//   node scripts/dev/verify-scenarios.mjs --headed       # 显示浏览器
//
// 场景：
//   1 配置快切忽略同名节点旧结果（切配置后迟到的测速结果不得覆盖新快照）
//   2 测速取消后停止派发（已取消后不再发起新测量，已完成结果保留）
//   3 诊断原样本/恢复样本区分 + 导出白名单（故障证据与摘要只含白名单字段）
//   4 默认/最小窗口控件可见性（中英文 × 明暗主题 × 1120x720/880x560）
//   5 异步失败通知归属、恢复与结构化端口引导
//   6 监听注册失败在挂载期间处理
//   7 语言切换后的既有轮询使用新语言
//
// 失败时把截图、IPC 调用计数、控制台错误写入临时目录并打印路径。

import {
  DESKTOP_DIR,
  artifactsDir,
  launchBrowser,
  sleep,
  revision,
  startVite,
  stubSource,
} from "./verify-lib.mjs";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

function parseArgs(args) {
  let scenario = null;
  let headed = false;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--headed") {
      headed = true;
    } else if (args[i] === "--scenario" && scenario === null && /^[1-7]$/.test(args[i + 1] ?? "")) {
      scenario = Number(args[++i]);
    } else {
      throw new Error("用法：pnpm verify:desktop [--scenario 1|2|3|4|5|6|7] [--headed]");
    }
  }
  return { scenario, headed };
}

/** 轮询直到 fn(page) 为真；超时抛出带描述的失败。 */
async function eventually(page, desc, fn, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < deadline) {
    try {
      const value = await fn(page);
      if (value) return value;
      last = "条件仍为假";
    } catch (e) {
      last = e?.message ?? String(e);
    }
    await sleep(150);
  }
  throw new Error(`等待超时（${timeoutMs}ms）：${desc}；最后状态：${String(last).slice(0, 300)}`);
}

function assert(cond, desc) {
  if (!cond) throw new Error(`断言失败：${desc}`);
}

async function calls(page, cmd) {
  return page.evaluate((c) => window.__cb.calls[c] ?? 0, cmd);
}

async function assertNoUnknownCommands(page) {
  const unknown = await page.evaluate(() => window.__cb.unknown);
  assert(unknown.length === 0, `出现未注册的 IPC 命令（夹具缺口）：${JSON.stringify(unknown.slice(0, 3))}`);
}

/** 打开应用并等启动完成（页脚版本号是 boot 完成标记）。 */
async function openApp(page, url) {
  await page.goto(url);
  await page.locator(".cb-system-foot", { hasText: "Crossbow v" }).waitFor({ state: "visible", timeout: 20_000 });
  await page.locator(".cb-primary-nav button").first().waitFor({ state: "visible", timeout: 10_000 });
  await assertNoUnknownCommands(page);
}

async function gotoNav(page, label, viewClass) {
  const button = page.locator(".cb-primary-nav button", { hasText: label });
  await button.waitFor({ state: "visible", timeout: 8000 });
  await button.click();
  await page.locator(viewClass).waitFor({ state: "visible", timeout: 8000 });
}

const nodeCard = (page, name) =>
  page.locator(".cb-node-card", { hasText: name }).first();

// Hold individual IPC replies until a scenario has observed the state it
// needs. A slow CI click can never let a held reply win the race.
async function holdMeasurements(page, delayBase) {
  await page.evaluate((base) => {
    const pending = [];
    let responded = 0;
    window.__cb.setCommands({
      test_node_delay_structured: (args) => new Promise((resolve) => {
        pending.push(() => {
          const number = Number(args.node.split("-").at(-1));
          resolve({ class: "ok", delayMs: base + number, reviewed: false,
            target: "https://stub.invalid/g", testedAtMs: Date.now(), sampleCount: 1, detail: "" });
          responded += 1;
        });
      }),
    });
    window.__cb.releaseMeasurements = (limit = pending.length) => {
      pending.splice(0, limit).forEach((reply) => reply());
    };
    window.__cb.measurementReplies = () => responded;
  }, delayBase);
}

// ---------------------------------------------------------------- 场景 1
async function scenarioProfileSwitch(page) {
  const mkNode = (name, delay_ms) => ({ name, kind: "ss", alive: delay_ms === null ? true : delay_ms > 0, delay_ms });
  const groupOf = (nodes) => [{ name: "出口", kind: "Selector", now: "🇭🇰 出口-01", nodes }];
  const snapshotA = { context: { revision: 1, pid: 4242 }, groups: groupOf([
    mkNode("🇭🇰 出口-01", 66), mkNode("🇯🇵 出口-02", null), mkNode("🇸🇬 出口-03", 140),
  ]) };
  const snapshotB = { context: { revision: 2, pid: 4242 }, groups: groupOf([
    mkNode("🇭🇰 出口-01", 88), mkNode("🇯🇵 出口-02", 90), mkNode("🇸🇬 出口-03", null),
    mkNode("新配置节点", 92),
  ]) };

  await holdMeasurements(page, 776);
  await page.evaluate(({ a, b }) => {
    let switched = false;
    window.__cb.setCommands({
      proxies_test_snapshot: () => switched ? b : a,
      set_active_profile: ({ id }) => {
        if (id !== "p-b") throw new Error("unexpected profile selection");
        switched = true;
        window.__cb.setCommands({ active_profile_id: id });
        window.__cb.emit("profile://switched", id);
      },
    });
  }, { a: snapshotA, b: snapshotB });

  await gotoNav(page, "节点", ".cb-view-proxies");
  const testAll = page.locator(".cb-node-test-all");
  await testAll.waitFor({ state: "visible", timeout: 8000 });
  await testAll.click();

  // 三个节点全部进入测量队列（并发 10 > 3，一次全发）
  await eventually(page, "3 个节点的测量请求全部发出",
    async (p) => (await calls(p, "test_node_delay_structured")) === 3);
  await nodeCard(page, "出口-01").waitFor({ state: "visible" });
  assert((await nodeCard(page, "出口-01").textContent()).includes("测速中"), "测量进行中应显示「测速中…」");

  await page.locator(".cb-source-pick").click();
  await page.locator(".cb-profile-switch-option", { hasText: "验证订阅B" }).click();
  // A newly added node proves snapshot B is rendered while shared cards
  // still carry the previous task's in-flight indicator.
  await nodeCard(page, "新配置节点").waitFor({ state: "visible" });
  await eventually(page, "页头配置名应切到订阅 B",
    async (p) => (await p.locator(".cb-source-pick").textContent()).includes("验证订阅B"));
  assert((await calls(page, "set_active_profile")) === 1, "应从页头发起一次配置切换");
  await eventually(page, "后端收到结束旧测速任务的请求",
    async (p) => (await calls(p, "end_node_delay_test")) >= 1);
  await page.evaluate(() => window.__cb.releaseMeasurements());
  await eventually(page, "旧测量结果返回且测速按钮退出任务",
    async (p) => (await p.evaluate(() => window.__cb.measurementReplies())) === 3
      && (await p.locator(".cb-node-test-all").textContent()).includes("延迟测试"));
  assert((await nodeCard(page, "出口-01").textContent()).includes("88 ms"),
    "旧结果（777 ms）返回后，应保留订阅 B 的 88 ms");
  assert((await calls(page, "test_node_delay_structured")) === 3, "切换后不应继续派发旧测量");
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 场景 2
async function scenarioCancelStopsDispatch(page) {
  const nodes = Array.from({ length: 24 }, (_, i) => ({
    name: `节点-${String(i + 1).padStart(2, "0")}`, kind: "ss", alive: true, delay_ms: 50,
  }));
  const snapshot = { context: { revision: 1, pid: 4242 }, groups: [
    { name: "出口", kind: "Selector", now: "节点-01", nodes },
  ] };

  await holdMeasurements(page, 100);
  await page.evaluate((data) => {
    window.__cb.setCommands({ proxies_test_snapshot: () => data });
  }, snapshot);

  await gotoNav(page, "节点", ".cb-view-proxies");
  const testAll = page.locator(".cb-node-test-all");
  await testAll.click();
  await eventually(page, "第一批测量已发出",
    async (p) => (await calls(p, "test_node_delay_structured")) >= 7);
  const snapshotReads = await calls(page, "proxies_test_snapshot");
  const dispatched = await calls(page, "test_node_delay_structured");
  await eventually(page, "测量中仍周期刷新代理组快照",
    async (p) => (await calls(p, "proxies_test_snapshot")) > snapshotReads);
  assert((await testAll.textContent()).includes("取消测速"), "周期刷新后任务应继续存在");

  await page.evaluate(() => window.__cb.releaseMeasurements(6));
  await eventually(page, "周期刷新后继续派发并保留完成项",
    async (p) => (await calls(p, "test_node_delay_structured")) >= dispatched + 6
      && (await nodeCard(p, "节点-01").textContent()).includes("101 ms"));
  const dispatchedAtCancel = await calls(page, "test_node_delay_structured");
  assert(dispatchedAtCancel < 24, "取消前必须还有未派发节点");
  await page.locator(".cb-node-test-all", { hasText: "取消测速" }).click();
  await eventually(page, "取消通知送达且未完成节点显示已取消",
    async (p) => (await calls(p, "end_node_delay_test")) >= 1
      && (await nodeCard(p, "节点-07").textContent()).includes("已取消"));

  await page.evaluate(() => window.__cb.releaseMeasurements());
  await eventually(page, "已派发结果全部返回且按钮复位",
    async (p) => (await p.evaluate(() => window.__cb.measurementReplies())) === dispatchedAtCancel
      && (await testAll.textContent()).includes("延迟测试"));
  assert((await calls(page, "test_node_delay_structured")) === dispatchedAtCancel,
    "迟到结果返回后也不能继续派发");
  assert((await nodeCard(page, "节点-01").textContent()).includes("101 ms"),
    "已完成节点的测速结果应保留（101 ms）");
  assert((await nodeCard(page, "节点-07").textContent()).includes("已取消"),
    "迟到结果不得把「已取消」改写成其他状态");
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 场景 3
async function scenarioDiagnostics(page) {
  const T0 = Date.now();
  const ctx = {
    os: "macos", os_version: "27.2.0", engine: "mihomo", core_state: "running",
    core_version: "1.19.32", proxy: "configured", tun: "off", mixed_port: 7897,
  };
  const row = (id, ok, detail, fix = null) => ({ id, ok, skipped: false, detail, fix });
  const reportIssue = {
    rows: [
      row("core_running", true, ""), row("controller", true, ""), row("config_renders", true, ""),
      row("listener", false, "127.0.0.1:7897 connect refused SECRET-RAW-ERR", "restart_core"),
      row("sysproxy", true, ""), row("proxied_traffic", false, "probe via mixed port SECRET-RAW-ERR"),
      row("dns_direct", true, ""),
    ],
    context: ctx,
  };
  const reportOk = { rows: reportIssue.rows.map((r) => ({ ...r, ok: true, detail: "", fix: null })), context: ctx };
  const probeOk = { class: "ok", httpStatus: 204 };
  const faultRecovered = {
    mode: "sysproxy", startedAtMs: T0 - 2_400_000, resolvedAtMs: T0 - 2_280_000,
    recovery: { atMs: T0 - 2_280_000, proxyHttps: { primary: probeOk, review: probeOk } },
    superseded: false, legacy: false, profileId: "p-a", mixedPort: 7897, mixedPortOk: true,
    sysproxyObserved: ["PAC=http://10.8.0.1:9877/pac?token=SECRET-TOKEN"],
    controller: { class: "ok", httpStatus: 200 }, directPath: "DIRECT",
    directCn: { primary: probeOk, review: null }, directGlobal: { primary: probeOk, review: null },
    proxyHttps: { primary: probeOk, review: probeOk },
  };
  const faultOngoing = {
    ...faultRecovered, startedAtMs: T0 - 300_000, resolvedAtMs: null, recovery: null,
    sysproxyObserved: [], proxyHttps: { primary: { class: "connect", httpStatus: null }, review: null },
  };
  const steps = ["core_running", "controller", "config_renders", "listener", "sysproxy", "proxied_traffic", "dns_direct"];

  await page.evaluate((data) => {
    window.__cb.setCommands({
      sysproxy_fault_evidence: () => ({ latest: data.faultRecovered, previous: data.faultOngoing }),
      run_diagnosis: async (args) => {
        for (const id of data.steps) {
          window.__cb.emit("diagnosis://progress", { run_id: args.runId, id, result: null });
          await new Promise((r) => setTimeout(r, 40));
          window.__cb.emit("diagnosis://progress", {
            run_id: args.runId, id,
            result: data.report.rows.find((x) => x.id === id),
          });
          await new Promise((r) => setTimeout(r, 30));
        }
        await new Promise((r) => setTimeout(r, 120));
        return data.report;
      },
    });
    window.__cb.finishAllOk = () => {
      window.__cb.setCommands({ run_diagnosis: () => data.reportOk });
    };
  }, { report: reportIssue, reportOk, faultRecovered, faultOngoing, steps });

  await page.locator(".cb-diagnostics-button").click();
  await page.locator(".cb-view-diagnostics").waitFor({ state: "visible" });

  // 故障证据：最新一条带恢复探针（恢复样本），上一条无恢复（原样本）
  const faultCard = page.locator(".cb-fault-evidence");
  await faultCard.waitFor({ state: "visible", timeout: 8000 });
  assert((await page.locator(".cb-fault-evidence .cb-fault-state").first().textContent()).includes("已验证恢复"),
    "最新故障样本应显示「已验证恢复」（含恢复探针）");
  // 改版后取证默认折叠：先展开卡片，再展开上一条（原样本）
  await page.locator(".cb-fault-evidence-toggle").click();
  await page.locator(".cb-fault-evidence details summary").first().click();
  await eventually(page, "上一条故障记录（原样本）展开可见",
    async (p) => (await p.locator(".cb-fault-evidence details").first().textContent()).includes("进行中"));
  // 页面可以展示观测值，但导出摘要必须白名单化
  assert((await faultCard.textContent()).includes("SECRET-TOKEN"), "页面本身应显示系统代理观测值");

  // 首次诊断：2 项失败（空状态 CTA 触发，动作唯一出口已移入状态带）
  await page.locator(".cb-empty .cb-button", { hasText: "开始诊断" }).click();
  await eventually(page, "诊断完成并提示 2 项需要处理",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("2 项需要处理"), 20_000);
  assert(await page.locator("#cb-check-listener.has-issue").isVisible(), "失败检查行应有 has-issue 标记");
  const firstFailed = page.locator("#cb-check-listener.first-failed");
  assert(await firstFailed.isVisible(), "首个阻塞项应在列表内升级显示");
  assert((await firstFailed.textContent()).includes("混合端口监听"),
    "首个阻塞项应指向混合端口监听");
  assert((await firstFailed.locator(".cb-prio-tag").textContent()).includes("优先处理"),
    "首个阻塞项应带优先处理标");

  // Capture the actual copy-button output without writing to the OS clipboard.
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async (text) => { window.__cb.copiedSummary = text; } },
    });
  });
  await page.locator(".cb-diag-band-actions button", { hasText: "预览诊断摘要" }).click();
  const share = await page.locator(".cb-share-preview").textContent();
  for (const must of [
    "Crossbow 0.14.4-verify", "Mixed port: 7897",
    "- Local mixed port could not be reached",
    "System proxy fault evidence (explicit mixed-port HTTPS; node unknown)",
    "State: recovered (verified)", "State: ongoing",
    // 「上一条故障记录」段首已走 i18n（#563）：夹具默认 zh，取中文值断言
    "上一条故障记录",
  ]) {
    assert(share.includes(must), `导出摘要应包含白名单字段：${must}`);
  }
  for (const banned of ["SECRET-TOKEN", "SECRET-RAW-ERR", "stub.invalid", "验证订阅", "节点-01"]) {
    assert(!share.includes(banned), `导出摘要泄露了非白名单内容：${banned}`);
  }
  await page.locator(".cb-dialog .actions button").click();
  await eventually(page, "复制入口输出和预览完全一致",
    (p) => p.evaluate((expected) => window.__cb.copiedSummary === expected, share));
  await page.keyboard.press("Escape");

  // 恢复复检：状态事件推进诊断序号 → 原样本标记过期 → 重跑全过（恢复样本）
  await page.evaluate(() => window.__cb.emit("core://status", { state: "Running" }));
  await eventually(page, "原样本应标记为已过期",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("上次检查结果已过期"));
  await page.evaluate(() => window.__cb.finishAllOk());
  await page.locator(".cb-diag-band-actions button", { hasText: "重新检查" }).click();
  await eventually(page, "复检全部通过",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("所有适用检查已通过"), 20_000);
  assert(!(await page.locator("#cb-check-listener").getAttribute("class")).includes("has-issue"),
    "复检通过后失败标记应清除");
  assert((await calls(page, "run_diagnosis")) === 2, "应恰好执行两次诊断");
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 场景 4
async function scenarioWindowControls(page) {
  const [{ width, height, minWidth, minHeight }] = JSON.parse(
    readFileSync(path.join(DESKTOP_DIR, "src-tauri/tauri.conf.json"), "utf8"),
  ).app.windows;
  const sizes = [{ width, height }, { width: minWidth, height: minHeight }];
  const views = ["home", "proxies", "connections", "profiles", "settings"];
  const languages = {
    zh: ["概览", "节点", "监测", "配置", "设置"],
    en: ["Overview", "Nodes", "Monitor", "Configuration", "Settings"],
  };
  const withinViewport = (box, size) => box && box.x >= -0.5 && box.y >= -0.5
    && box.x + box.width <= size.width + 0.5 && box.y + box.height <= size.height + 0.5;

  for (const size of sizes) {
    await page.setViewportSize(size);
    for (const [lang, labels] of Object.entries(languages)) {
      for (const theme of ["light", "dark"]) {
        const label = `${size.width}×${size.height} · ${lang} · ${theme}`;
        await page.evaluate(({ lang, theme }) => {
          window.__cb.emit("ui://lang", lang);
          window.__cb.emit("ui://theme", theme);
        }, { lang, theme });
        await eventually(page, `应用语言和主题 ${label}`, async (p) =>
          await p.locator(".cb-primary-nav button", { hasText: labels[0] }).isVisible()
          && (await p.evaluate(() => document.documentElement.classList.contains("dark"))) === (theme === "dark"));
        for (const [index, name] of labels.entries()) {
          const button = page.locator(".cb-primary-nav button", { hasText: name });
          assert(withinViewport(await button.boundingBox(), size), `${label} 下「${name}」越界`);
          await gotoNav(page, name, `.cb-view-${views[index]}`);
        }
        assert(withinViewport(await page.locator(".cb-system-foot").boundingBox(), size),
          `${label} 下页脚越界`);

        const portButton = page.locator("#cb-port-setting .cb-port-readout button");
        await eventually(page, `混合端口编辑已就绪 ${label}`, () => portButton.isEnabled());
        await page.locator(".cb-primary-nav button", { hasText: labels.at(-1) }).focus();
        let reached = false;
        for (let step = 0; step < 30; step++) {
          await page.keyboard.press("Tab");
          reached = await portButton.evaluate((button) => button === document.activeElement);
          if (reached) break;
        }
        assert(reached && withinViewport(await portButton.boundingBox(), size),
          `${label} 下应能 Tab 到混合端口，且随键盘焦点滚动到视口`);
        await page.keyboard.press("Enter");
        const field = page.locator("#mixed-port");
        await field.waitFor({ state: "visible" });
        assert(await field.evaluate((input) => input === document.activeElement),
          `${label} 下键盘打开编辑后应聚焦端口输入框`);
        await page.keyboard.press("Escape");
        await eventually(page, `关闭编辑后焦点回到端口按钮 ${label}`,
          () => portButton.evaluate((button) => button === document.activeElement));
      }
    }
  }
  await assertNoUnknownCommands(page);
}


// #562/#563：失败后的通知归属、恢复与结构化端口引导。
async function scenarioAsyncErrors(page) {
  await page.evaluate(() => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__cb.notices = [];
    window.__cb.resolved = [];
    window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      if (cmd === "notification_publish") window.__cb.notices.push(args.notice);
      if (cmd === "notification_resolve") window.__cb.resolved.push(args.source);
      return invoke(cmd, args);
    };
    window.__cb.setCommands({ list_overrides: () => { throw new Error("override read failed"); } });
  });
  await gotoNav(page, "配置", ".cb-view-profiles");
  await page.getByRole("button", { name: "覆写链", exact: true }).click();
  await page.getByText("覆写读取失败，请重试", { exact: true }).waitFor();
  const dir = artifactsDir();
  await sleep(350);
  await page.screenshot({ path: path.join(dir, "override-load-error.png"), fullPage: true });
  await page.setViewportSize({ width: 880, height: 560 });
  await page.evaluate(() => window.__cb.emit("ui://theme", "dark"));
  await sleep(350);
  await page.screenshot({ path: path.join(dir, "override-load-error-dark.png"), fullPage: true });
  assert(await page.getByRole("button", { name: "重试", exact: true }).isVisible(), "最小窗口深色主题的重试入口可见");
  await page.setViewportSize({ width: 1120, height: 720 });
  await page.evaluate(() => window.__cb.emit("ui://theme", "light"));
  await page.evaluate(() => window.__cb.setCommands({ list_overrides: [] }));
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await eventually(page, "覆写读取成功解除旧失败", () => page.evaluate(() =>
    window.__cb.resolved.includes("override-load")));

  await page.locator(".cb-diagnostics-button").click();
  const steps = ["core_running", "controller", "config_renders", "listener", "sysproxy", "proxied_traffic", "dns_direct"];
  for (const [fix, command, id] of [
    ["restart_core", "core_restart", "listener"],
    ["enable_sysproxy", "diagnosis_enable_sysproxy", "sysproxy"],
    ["start_core", "core_start", "core_running"],
  ]) {
    await page.evaluate(({ steps, fix, command, id }) => {
      window.__cb.notices = [];
      window.__cb.setCommands({
        run_diagnosis: {
          rows: steps.map(key => ({ id: key, ok: key !== id, skipped: false, detail: "", fix: key === id ? fix : null })),
          context: { os: "macos", os_version: "27.2", engine: "mihomo", core_state: "running",
            core_version: "stub", proxy: "off", tun: "off", mixed_port: 7897 },
        },
        [command]: () => { throw new Error("repair failed"); },
      });
    }, { steps, fix, command, id });
    const initial = page.locator(".cb-view-diagnostics .cb-empty button");
    if (await initial.count()) await initial.click();
    else await page.locator(".cb-diag-band-actions button").last().click();
    const repair = page.locator(`#cb-check-${id} button`);
    await repair.waitFor();
    const before = await calls(page, "run_diagnosis");
    await repair.click();
    await eventually(page, `${fix} 失败仍复检`, async () => (await calls(page, "run_diagnosis")) > before);
    const count = await page.evaluate(() => window.__cb.notices.filter(n => n.source === "diagnostics-fix").length);
    assert(count === (fix === "start_core" ? 1 : 0), `${fix} 通知由唯一归属方报告（实际前端 ${count} 条）`);
  }

  await gotoNav(page, "概览", ".cb-view-home");
  await page.evaluate(() => {
    localStorage.setItem("crossbow:first-connection", "pending");
    window.__cb.setCommands({ core_status: { state: "Stopped" }, tun_status: { enabled: false, ready: false, coreRunning: false } });
    window.__cb.emit("core://status", { state: "Stopped" });
  });
  // remount so the setup progress is read from storage.
  await gotoNav(page, "配置", ".cb-view-profiles");
  await gotoNav(page, "概览", ".cb-view-home");
  const start = page.getByRole("button", { name: "启动内核", exact: true });
  await start.waitFor();
  for (const [error, conflict] of [
    [{ kind: "portUnavailable", port: 7897, message: "bind failed in any language" }, true],
    [{ kind: "startFailed", port: null, message: "无法绑定 is not a failure code" }, false],
  ]) {
    await page.evaluate(error => window.__cb.setCommands({ core_start: () => Promise.reject(error) }), error);
    await start.click();
    await eventually(page, "启动失败后按钮恢复", () => start.isEnabled());
    const hint = page.getByText("混合端口无法绑定，请在网络设置中修改端口后重试。", { exact: true });
    assert(await hint.isVisible() === conflict, "仅结构化绑定失败显示修改端口引导");
    assert(!await page.evaluate(() => window.__cb.notices.some(n => n.details?.includes("[object Object]"))), "通知保留可读的结构化错误详情");
  }
  await page.evaluate(() => window.__cb.setCommands({ get_engine_config: () => Promise.reject("engine read failed") }));
  await gotoNav(page, "节点", ".cb-view-proxies");
  await page.getByRole("heading", { name: "内核尚未就绪", exact: true }).waitFor();
  await eventually(page, "次要读取失败局部提示", () => page.evaluate(() => window.__cb.notices.some(n => n.source === "proxies-engine-read")));
  await page.evaluate(() => window.__cb.setCommands({ get_engine_config: { core_installed: false } }));
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await page.getByRole("heading", { name: "内核未安装", exact: true }).waitFor();
  await eventually(page, "次要读取成功解除错误", () => page.evaluate(() => window.__cb.resolved.includes("proxies-engine-read")));
  await assertNoUnknownCommands(page);
  console.log(`  生产覆写页失败态截图：${dir}/override-load-error.png`);
}


async function scenarioRegistrationFailure(page) {
  await page.addInitScript(() => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      if (cmd === "plugin:event|listen" && ["ui://theme", "ui://goto", "engine://changed", "traffic://data"].includes(args.event)) {
        return Promise.reject(new Error("registration failed"));
      }
      return invoke(cmd, args);
    };
  });
  await page.reload();
  await page.locator(".cb-system-foot").waitFor();
  // Stay mounted long enough for registration rejection to be observed;
  // catching only during unmount cannot satisfy this scenario.
  await sleep(150);
  await gotoNav(page, "配置", ".cb-view-profiles");
  await gotoNav(page, "概览", ".cb-view-home");
  await sleep(150);
  await assertNoUnknownCommands(page);
}


async function scenarioPollingLanguage(page) {
  await page.clock.install();
  await page.reload();
  await page.locator(".cb-system-foot").waitFor();
  await page.evaluate(() => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    window.__cb.notices = [];
    window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      if (cmd === "notification_publish") window.__cb.notices.push(args.notice);
      return invoke(cmd, args);
    };
    window.__cb.setCommands({ coexist_detect: [{ kind: "tunnel", detail: "stub tunnel" }] });
    window.__cb.emit("ui://lang", "en");
  });
  await page.locator(".cb-primary-nav button", { hasText: "Overview" }).waitFor();
  await page.clock.fastForward(60_001);
  await eventually(page, "已挂载轮询读取当前语言", () => page.evaluate(() =>
    window.__cb.notices.some(n => n.source === "coexist" && n.title === "Another proxy is running")));
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 编排
const SCENARIOS = [
  { id: 1, name: "配置快切忽略同名节点旧结果", fn: scenarioProfileSwitch },
  { id: 2, name: "测速取消后停止派发", fn: scenarioCancelStopsDispatch },
  { id: 3, name: "诊断原样本/恢复样本与导出白名单", fn: scenarioDiagnostics },
  { id: 4, name: "默认/最小窗口控件可见性（中英文 × 明暗）", fn: scenarioWindowControls },
  { id: 5, name: "异步失败通知归属、恢复与端口引导", fn: scenarioAsyncErrors },
  { id: 6, name: "监听注册失败在挂载期间处理", fn: scenarioRegistrationFailure },
  { id: 7, name: "语言切换后的既有轮询使用新语言", fn: scenarioPollingLanguage },
];

async function main() {
  const { scenario: selected, headed } = parseArgs(process.argv.slice(2));
  const commit = revision();
  const vite = await startVite();
  const origin = new URL(vite.url).origin;
  let browser;
  let dir;
  let failures = 0;
  let passedCount = 0;
  try {
    browser = await launchBrowser(!headed);
    const browserVersion = browser.version();
    console.log(`桌面模拟 · commit ${commit} · ${process.platform}/${process.arch} · Chromium ${browserVersion}`);
    for (const scenario of SCENARIOS) {
      if (selected !== null && scenario.id !== selected) continue;
      const context = await browser.newContext({ viewport: { width: 1120, height: 720 }, serviceWorkers: "block" });
      try {
        // Production UI may grow network features. Keep the browser preview
        // on loopback even if a newly added feature bypasses the IPC mock.
        const blockedRequests = [];
        await context.route("**/*", (route) => {
          if (new URL(route.request().url()).origin === origin) return route.continue();
          blockedRequests.push(route.request().resourceType());
          return route.abort();
        });
        await context.routeWebSocket(/.*/, (socket) => {
          const endpoint = new URL(socket.url());
          if (endpoint.protocol === "ws:" && endpoint.host === new URL(origin).host) socket.connectToServer();
          else {
            blockedRequests.push("websocket");
            socket.close();
          }
        });
        await context.addInitScript(stubSource());
        const page = await context.newPage();
        const consoleErrors = [];
        const pageErrors = [];
        page.on("console", (msg) => {
          if (msg.type() === "error" && consoleErrors.length < 20) consoleErrors.push(msg.text().slice(0, 300));
        });
        page.on("pageerror", (err) => {
          if (pageErrors.length < 20) pageErrors.push(String(err).slice(0, 300));
        });
        const started = Date.now();
        try {
          await openApp(page, vite.url);
          await scenario.fn(page);
          assert(pageErrors.length === 0, `页面脚本错误：${pageErrors.join(" | ")}`);
          assert(blockedRequests.length === 0, `页面尝试访问非本机地址：${blockedRequests.join(", ")}`);
          passedCount += 1;
          console.log(`✓ 场景 ${scenario.id} ${scenario.name}（${((Date.now() - started) / 1000).toFixed(1)}s）`);
        } catch (error) {
          failures += 1;
          dir ??= artifactsDir();
          const name = path.join(dir, `scenario-${scenario.id}`);
          await page.screenshot({ path: `${name}.png`, fullPage: true, timeout: 5000 }).catch(() => {});
          const ipc = await page.evaluate(() => ({
            calls: window.__cb?.calls, unknown: window.__cb?.unknown,
          })).catch(() => ({}));
          writeFileSync(`${name}.json`, JSON.stringify({
            scenario: scenario.name, commit, browserVersion, platform: process.platform,
            viewport: page.viewportSize(), error: error.message, ipc, consoleErrors, pageErrors, blockedRequests,
          }, null, 2));
          console.error(`✗ 场景 ${scenario.id} ${scenario.name}（${commit}）：${error.message}\n    证据目录：${dir}`);
        }
      } finally {
        await context.close();
      }
    }
  } finally {
    try {
      if (browser) await browser.close();
    } finally {
      await vite.close();
    }
  }
  console.log(`通过 ${passedCount} 项${failures ? `，失败 ${failures} 项` : "，全部通过"}`);
  if (failures) process.exitCode = 1;
}

main().catch((error) => {
  console.error("FATAL:", error.message);
  process.exitCode = 2;
});
