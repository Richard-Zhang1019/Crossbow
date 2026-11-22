# 统一 Modal v03：#638 评审修复

2026-11-22，#631 / PR #638。基线 PR head `fc937ad77e7dc3ef1eb7b3374aa5e39d706849cb`；评审期间同步 main `761af5e`（#636），合并提交 `8cfaa82`。保留 v01/v02，不覆盖旧稿。

## 问题核验与方案

在 PR 的 merge-base `3bb2c20` 核实 #631 所述缺口真实存在：旧 Dialog 只有标题、关闭和正文，14 处调用的等待/底栏各自组织，标题 ID 固定且整个弹窗滚动。PR 使用现有 Base UI、受控 Modal API 与公共 Button 的方向，与评审推荐方案一致；比继续扩展手写焦点循环更合适，不需要引入完整 antd。

| Before | After | Why |
| --- | --- | --- |
| 长标题自然增高，header/footer 不收缩，但 popup 裁切 | 标题视觉最多三行，完整 DOM 文本/可访问名称保留；字符串标题提供原生悬停全文 | 880×560、360 字标题时，修复前 footer bottom=587 超出 popup bottom=536，保存/取消被裁切；操作区应有稳定预算 |
| NotificationProvider 没有常驻 live region，Modal 开启后应用根被背景隔离 | 常驻 polite live region 承载原 NotificationViewport，保留每张卡原有 alert/status 和统一服务 | Modal 打开后才到达的失败通知不再继承 aria-hidden；不是把整个应用背景解除隔离，也不增加第二套报错弹窗 |
| #636 在评审期间进入 main，CI 清单/规范/设计索引发生冲突 | 保留两套回归入口与文档，自动合并后的 Diagnostics 同时保留新布局和摘要 Modal | 不能为统一 Modal 回退诊断页布局，也不能重新引入已删除的 Dialog |

没有改变保存结果、后端状态、网络接管、取消协议或 UI 色板。NotificationViewport 的显示/倒计时逻辑不变，空 live region 不占页面内容高度。最小窗口的标题截断通过完整可访问名称和字符串 hover 提供全文，不裁切操作区。

## 验证

- `npm run build --prefix apps/desktop`：TypeScript/Vite 通过，保留既有大包提示。
- Node 24 前端单测 47/47。
- 扩展 `verify-modal.mjs`：既有异步失败留草稿、等待关闭保护、兄弟/嵌套/Select 的 Esc、双向 Tab、焦点恢复、隐藏保留与回调继续通过；新增通知晚到后的可访问树、点击详情不关草稿、长标题下固定动作真实命中。
- 通知使用真实生产 NotificationProvider/Card，夹具只发脱敏 app://notifications 快照；测试不是自制通知替身。验证 live region/alert 没有 aria-hidden 祖先，不宣称已完成 VoiceOver/NVDA 实际播报验收。
- 合并后 `verify-scenarios.mjs` 十项全部通过；`verify-diagnostics-layout.mjs` 110 项布局/状态采样通过。
- 配置包生产回归和分项测试生产回归通过：关闭释放预览、失败/冲突处理、关闭取消分项任务、迟到票据取消、小窗口暗色英文等。两项测试在 main 同步前执行，相关消费代码/取消协议在同步中没有变化。
- 所有场景为隔离浏览器 + 生产组件 + 合成 IPC，未改动真实代理、订阅、TUN 或已安装应用。原生 Tauri/WebView、VoiceOver/NVDA、Windows/Linux 实机与修复提交的 CI 仍待验。

## 预览来源

- `before-long-title-light.png`：原 PR fc937ad 的生产 Modal，880×560，360 字标题。
- `before-notification-light.png`：同基线，真实通知已显示，但其应用根祖先 aria-hidden=true；该问题靠可访问树记录核验，不能只看截图。
- `after-long-title-dark.png`：合并 main 后的生产 Modal，同为 880×560、360 字标题；暗色用于补充主题覆盖，不能把配色差异当作本次设计变更。
- `after-notification-light.png`：合并后真实 Modal/NotificationProvider，1120×734；详情可打开、草稿保留。
- `verification/before.json` 记录已复现几何与 aria-hidden；`modal.json`/`diagnostics.json` 为最终工作树的回归输出，完整源稿哈希在 manifest；脚本输出中的 8cfaa82 是合并基线，尚未包含未提交修复，不能单独当作完整源码版本。

`source.zip` 是可编辑生产组件、样式、通知承载、回归夹具和 API 文档快照。在相同依赖锁文件的仓库解压后，运行 `node scripts/dev/verify-modal.mjs`；可用 `CB_MODAL_EVIDENCE` 指定截图目录。原 API/迁移决定见 [MODAL.md](../../../MODAL.md)。
