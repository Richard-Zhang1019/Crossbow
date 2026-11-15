#!/usr/bin/env node
// #511：可复跑桌面验证入口——生产组件 + IPC 桩，固化四个关键回归场景。
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
//
// 失败时把截图、IPC 调用计数、控制台错误写入临时目录并打印路径。

import {
  VERIFY_URL,
  artifactsDir,
  launchBrowser,
  sleep,
  startVite,
  stopVite,
  stubSource,
} from "./verify-lib.mjs";
import { writeFileSync } from "node:fs";

const scenarioArg = (() => {
  const i = process.argv.indexOf("--scenario");
  return i > -1 ? Number(process.argv[i + 1]) : null;
})();
const headed = process.argv.includes("--headed");

process.on("unhandledRejection", (e) => {
  console.error("未捕获异常：", e);
  process.exit(2);
});

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
async function openApp(page) {
  await page.goto(VERIFY_URL);
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

// ---------------------------------------------------------------- 场景 1
async function scenarioProfileSwitch(page) {
  const mkNode = (name, delay_ms) => ({ name, kind: "ss", alive: delay_ms === null ? true : delay_ms > 0, delay_ms });
  const groupOf = (nodes) => [{ name: "出口", kind: "Selector", now: "🇭🇰 出口-01", nodes }];
  const snapshotA = { context: { revision: 1, pid: 4242 }, groups: groupOf([
    mkNode("🇭🇰 出口-01", 66), mkNode("🇯🇵 出口-02", null), mkNode("🇸🇬 出口-03", 140),
  ]) };
  const snapshotB = { context: { revision: 2, pid: 4242 }, groups: groupOf([
    mkNode("🇭🇰 出口-01", 88), mkNode("🇯🇵 出口-02", 90), mkNode("🇸🇬 出口-03", null),
  ]) };

  await page.evaluate((data) => {
    let switched = false;
    window.__cb.setCommands({
      proxies_test_snapshot: () => (switched ? data.b : data.a),
      // 结果迟到 2.2s：制造「切配置后旧结果才回来」的窗口
      test_node_delay_structured: async () => {
        await new Promise((r) => setTimeout(r, 2200));
        return { class: "ok", delayMs: 777, reviewed: false, target: "https://stub.invalid/g", testedAtMs: Date.now(), sampleCount: 1, detail: "" };
      },
    });
    window.__cb.switchToB = () => {
      switched = true;
      window.__cb.setCommands({ active_profile_id: "p-b" });
    };
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

  // 切到订阅 B（同名节点、不同快照序号），旧结果 2.2s 后才回来
  await page.evaluate(() => { window.__cb.switchToB(); window.__cb.emit("profile://switched", "p-b"); });

  await eventually(page, "订阅 B 快照基线（88 ms）上屏",
    async (p) => (await nodeCard(p, "出口-01").textContent()).includes("88 ms"), 6000);
  await eventually(page, "页头配置名应切到 订阅B",
    async (p) => (await p.evaluate(() => document.body.textContent)).includes("验证订阅B"));
  const dispatched = await calls(page, "test_node_delay_structured");

  await sleep(2600); // 越过 2.2s 迟到窗口
  const pageText = await page.evaluate(() => document.body.textContent);
  assert(!pageText.includes("777 ms"), "迟到的旧测速结果（777 ms）不得覆盖新配置快照");
  assert(!pageText.includes("测速中"), "切配置后不应残留「测速中」状态");
  assert((await calls(page, "test_node_delay_structured")) === dispatched, "切换后不应有新的测量派发");
  assert((await calls(page, "end_node_delay_test")) >= 1, "切换后应通知后端结束旧测量任务");
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

  await page.evaluate((data) => {
    window.__cb.setCommands({
      proxies_test_snapshot: () => data.snapshot,
      // 前 6 次调用 300ms 返回（让队列继续派发），其余 1.6s（占住并发位）
      test_node_delay_structured: async (args, _cmd, seq) => {
        await new Promise((r) => setTimeout(r, seq <= 6 ? 300 : 1600));
        const n = Number(args.node.split("-")[1]);
        return { class: "ok", delayMs: 100 + n, reviewed: false, target: "https://stub.invalid/g", testedAtMs: Date.now(), sampleCount: 1, detail: "" };
      },
    });
  }, { snapshot });

  await gotoNav(page, "节点", ".cb-view-proxies");
  const testAll = page.locator(".cb-node-test-all");
  await testAll.waitFor({ state: "visible", timeout: 8000 });
  await testAll.click();

  // 等第二批派发发生，证明队列在持续运转
  await eventually(page, "测量队列持续派发（第二批已发出）",
    async (p) => (await calls(p, "test_node_delay_structured")) >= 12, 10_000);
  const dispatchedAtCancel = await calls(page, "test_node_delay_structured");
  assert(dispatchedAtCancel < 24, `取消时刻应仍有未派发节点（实际已派发 ${dispatchedAtCancel}/24）`);

  await page.locator(".cb-node-test-all", { hasText: "取消测速" }).click();

  await sleep(250);
  assert((await calls(page, "test_node_delay_structured")) === dispatchedAtCancel,
    "点击取消后不得再派发新的测量请求");
  assert((await nodeCard(page, "节点-07").textContent()).includes("已取消"),
  "未完成节点-07 应显示「已取消」");
  assert((await nodeCard(page, "节点-08").textContent()).includes("已取消"),
  "未完成节点-08 应显示「已取消」");

  await sleep(2400); // 越过 1.6s 迟到返回窗口
  assert((await calls(page, "test_node_delay_structured")) === dispatchedAtCancel,
    "取消后队列应完全停住，不再派发");
  assert((await nodeCard(page, "节点-01").textContent()).includes("101 ms"),
    "已完成节点的测速结果应保留（101 ms）");
  assert((await nodeCard(page, "节点-07").textContent()).includes("已取消"),
  "迟到结果不得把「已取消」改写成其他状态");

  await eventually(page, "任务收尾：按钮恢复为「延迟测试」",
    async (p) => (await p.locator(".cb-node-test-all").textContent()).includes("延迟测试"), 8000);
  assert((await calls(page, "end_node_delay_test")) >= 1, "取消后应通知后端结束测量任务");
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
  await page.locator(".cb-fault-evidence details summary").first().click();
  await eventually(page, "上一条故障记录（原样本）展开可见",
    async (p) => (await p.locator(".cb-fault-evidence details").first().textContent()).includes("进行中"));
  // 页面可以展示观测值，但导出摘要必须白名单化
  assert((await faultCard.textContent()).includes("SECRET-TOKEN"), "页面本身应显示系统代理观测值");

  // 首次诊断：2 项失败
  await page.locator(".cb-page-head button", { hasText: "开始诊断" }).click();
  await eventually(page, "诊断完成并提示 2 项需要处理",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("2 项需要处理"), 20_000);
  assert(await page.locator("#cb-check-listener.has-issue").isVisible(), "失败检查行应有 has-issue 标记");
  assert((await page.locator("#cb-check-first-problem").textContent()).includes("混合端口监听"),
    "首个阻塞项应指向混合端口监听");

  // 导出白名单：摘要 + 故障证据，不得含订阅、节点名、原始错误、观测值
  await page.locator(".cb-page-head button", { hasText: "预览诊断摘要" }).click();
  const share = await page.locator(".cb-share-preview").textContent();
  for (const must of [
    "Crossbow 0.14.4-verify", "Mixed port: 7897",
    "- Local mixed port could not be reached",
    "System proxy fault evidence (explicit mixed-port HTTPS; node unknown)",
    "State: recovered (verified)", "Previous fault record", "State: ongoing",
  ]) {
    assert(share.includes(must), `导出摘要应包含白名单字段：${must}`);
  }
  for (const banned of ["SECRET-TOKEN", "SECRET-RAW-ERR", "stub.invalid", "验证订阅", "节点-01"]) {
    assert(!share.includes(banned), `导出摘要泄露了非白名单内容：${banned}`);
  }
  await page.keyboard.press("Escape");

  // 恢复复检：状态事件推进诊断序号 → 原样本标记过期 → 重跑全过（恢复样本）
  await page.evaluate(() => window.__cb.emit("core://status", { state: "Running" }));
  await eventually(page, "原样本应标记为已过期",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("上次检查结果已过期"));
  await page.evaluate(() => window.__cb.finishAllOk());
  await page.locator(".cb-page-head button", { hasText: "重新检查" }).click();
  await eventually(page, "复检全部通过",
    async (p) => (await p.locator(".cb-diag-summary h2").textContent()).includes("所有适用检查已通过"), 20_000);
  assert(!(await page.locator("#cb-check-listener").getAttribute("class")).includes("has-issue"),
    "复检通过后失败标记应清除");
  assert((await calls(page, "run_diagnosis")) === 2, "应恰好执行两次诊断");
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 场景 4
async function scenarioWindowControls(page) {
  const sizes = [
    { w: 1120, h: 720, name: "默认 1120x720" },
    { w: 880, h: 560, name: "最小 880x560" },
  ];
  const navTargets = [
    ["概览", ".cb-view-home"],
    ["节点", ".cb-view-proxies"],
    ["监测", ".cb-view-connections"],
    ["配置", ".cb-view-profiles"],
    ["设置", ".cb-view-settings"],
  ];

  const checkNavBounds = async (clickOne) => {
    const vp = page.viewportSize();
    for (const [label] of navTargets) {
      const button = page.locator(".cb-primary-nav button", { hasText: label });
      await button.waitFor({ state: "visible", timeout: 8000 });
      const box = await button.boundingBox();
      assert(box, `导航「${label}」应有布局尺寸`);
      assert(box.x >= 0 && box.y >= 0 && box.x + box.width <= vp.width && box.y + box.height <= vp.height,
        `窗口 ${vp.width}x${vp.height} 下导航「${label}」超出视口：${JSON.stringify({ box, vp })}`);
    }
    if (clickOne) {
      // 拖拽层不得拦截导航点击（mac Overlay 标题栏回归面）
      await page.locator(".cb-primary-nav button", { hasText: "概览" }).click();
      await page.locator(".cb-view-home").waitFor({ state: "visible" });
    }
    const footer = page.locator(".cb-system-foot");
    await footer.waitFor({ state: "visible" });
    const footerBox = await footer.boundingBox();
    assert(footerBox.y + footerBox.height <= vp.height, `窗口 ${vp.width}x${vp.height} 下页脚溢出视口：${JSON.stringify(footerBox)}`);
  };

  await page.locator(".cb-system-foot").waitFor({ state: "visible", timeout: 15_000 });
  for (const size of sizes) {
    await page.setViewportSize({ width: size.w, height: size.h });
    await sleep(350); // 等布局/ResizeObserver 稳定
    for (const [label, view] of navTargets) {
      await gotoNav(page, label, view);
    }
    await checkNavBounds(true);
    // 键盘可达：Tab 后焦点落到可聚焦控件
    await page.evaluate(() => document.activeElement?.blur());
    await page.keyboard.press("Tab");
    const focusedTag = await page.evaluate(() => document.activeElement?.tagName ?? "NONE");
    assert(focusedTag === "BUTTON" || focusedTag === "A" || focusedTag === "INPUT",
      `Tab 后焦点应落在可交互控件上（实际 ${focusedTag}）`);
  }

  // 最小窗口下设置页端口设置可聚焦并进入视口（焦点/滚动可用）
  await gotoNav(page, "设置", ".cb-view-settings");
  const scrollInfo = await page.evaluate(() => {
    const row = document.getElementById("cb-port-setting");
    // ControlRow 是不可聚焦的 div；聚焦行内的实际控件（input/button）
    const control = row?.querySelector("input, button, select") ?? row;
    control?.scrollIntoView({ block: "center" });
    control?.focus({ preventScroll: true });
    const active = document.activeElement;
    const scrollers = [...document.querySelectorAll(".cb-paper, .cb-page-content")]
      .filter((e) => e.scrollHeight > e.clientHeight + 4);
    return {
      activeTag: active?.tagName ?? "NONE",
      activeInRow: !!(active && row && row.contains(active)),
      top: row ? Math.round(row.getBoundingClientRect().top) : -1,
      viewportH: window.innerHeight,
      scrollers: scrollers.map((e) => e.className),
    };
  });
  assert(scrollInfo.activeInRow && scrollInfo.activeTag !== "BODY" && scrollInfo.activeTag !== "NONE",
    `端口设置行内控件应可聚焦（实际 ${scrollInfo.activeTag}）`);
  assert(scrollInfo.top >= 0 && scrollInfo.top < scrollInfo.viewportH,
    `聚焦后端口设置应在视口内：${JSON.stringify(scrollInfo)}`);

  // 语言与主题热切换（设置事件路径，不重载页面）
  await page.evaluate(() => { window.__cb.emit("ui://lang", "en"); window.__cb.emit("ui://theme", "dark"); });
  await eventually(page, "切换英文后导航出现 Nodes",
    async (p) => p.locator(".cb-primary-nav button", { hasText: "Nodes" }).isVisible());
  assert(await page.evaluate(() => document.documentElement.classList.contains("dark")), "dark 主题应挂到 html.dark");
  const enVp = page.viewportSize();
  for (const label of ["Overview", "Nodes", "Monitor", "Configuration", "Settings"]) {
    const box = await page.locator(".cb-primary-nav button", { hasText: label }).boundingBox();
    assert(box && box.x >= 0 && box.y >= 0 && box.x + box.width <= enVp.width && box.y + box.height <= enVp.height,
      `英文导航「${label}」在最小窗口下超出视口：${JSON.stringify(box)}`);
  }
  const bgDark = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  await page.evaluate(() => window.__cb.emit("ui://theme", "light"));
  await eventually(page, "light 主题应移除 html.dark",
    async (p) => !((await p.evaluate(() => document.documentElement.classList.contains("dark")))));
  const bgLight = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  assert(bgDark !== bgLight, `明暗主题应有可见差异（${bgDark} vs ${bgLight}）`);
  await assertNoUnknownCommands(page);
}

// ---------------------------------------------------------------- 编排
const SCENARIOS = [
  { id: 1, name: "配置快切忽略同名节点旧结果", fn: scenarioProfileSwitch },
  { id: 2, name: "测速取消后停止派发", fn: scenarioCancelStopsDispatch },
  { id: 3, name: "诊断原样本/恢复样本与导出白名单", fn: scenarioDiagnostics },
  { id: 4, name: "默认/最小窗口控件可见性（中英文×明暗）", fn: scenarioWindowControls },
];

async function main() {
  const vite = await startVite();
  let browser;
  const dir = artifactsDir();
  const failures = [];
  let passedCount = 0;
  try {
    browser = await launchBrowser(!headed);
    for (const scenario of SCENARIOS) {
      if (scenarioArg !== null && scenario.id !== scenarioArg) continue;
      const context = await browser.newContext({ viewport: { width: 1120, height: 720 } });
      await context.addInitScript(stubSource());
      const page = await context.newPage();
      const consoleErrors = [];
      page.on("console", (msg) => {
        if (msg.type() === "error") consoleErrors.push(msg.text().slice(0, 300));
      });
      page.on("pageerror", (err) => consoleErrors.push(`pageerror: ${String(err).slice(0, 300)}`));
      const started = Date.now();
      try {
        await openApp(page);
        await scenario.fn(page);
        passedCount += 1;
        console.log(`✓ 场景${scenario.id} ${scenario.name}（${((Date.now() - started) / 1000).toFixed(1)}s）`);
      } catch (error) {
        const shot = `${dir}/scenario${scenario.id}.png`;
        await page.screenshot({ path: shot, fullPage: true }).catch(() => {});
        const dump = await page.evaluate(() => ({ calls: window.__cb.calls, unknown: window.__cb.unknown })).catch(() => ({}));
        writeFileSync(`${dir}/scenario${scenario.id}.calls.json`, JSON.stringify(dump, null, 2));
        const message = [
          `✗ 场景${scenario.id} ${scenario.name} 失败：${error.message}`,
          `    应用版本标记：页脚 Crossbow v…（stub 版本 0.14.4-verify）`,
          `    控制台错误（前 5 条）：${consoleErrors.slice(0, 5).join(" | ") || "无"}`,
          `    证据目录：${dir}`,
        ].join("\n");
        failures.push(message);
        console.error(message);
      } finally {
        await context.close();
      }
    }
  } finally {
    if (browser) await browser.close().catch(() => {});
    stopVite(vite);
  }
  console.log(`\n通过 ${passedCount} 项${failures.length ? `，失败 ${failures.length} 项` : "，全部通过"}`);
  if (failures.length) {
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("FATAL:", e.message);
  process.exit(2);
});
