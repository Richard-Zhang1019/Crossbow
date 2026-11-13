# 设计稿档案

用户要求：今后每次设计稿均留存文件，方便回看、比较和追溯。

## 保存规则

1. 每个主题使用 `YYYY-MM-DD-topic/` 目录，每次交付增加 `v01/`、`v02/`；新版不覆盖旧版。
2. 每版至少保留可编辑源稿、设计说明、实际渲染预览与 manifest。动效必须保留可播放原型；视频/GIF 在用户要求导出时一并保存。
3. 说明记录用户目标、关键参数、交互状态、验收边界和关联 issue/PR；标注「提案 / 已确认 / 已实现」及正式实现位置。
4. manifest 记录日期、版本、输入来源或摘要、基线提交和文件清单。资产依赖一同保存；不以临时链接、聊天截图或运行中的本地服务代替源文件。
5. 仅保留脱敏、可公开评审的示例；禁止把订阅 token、真实代理凭据、签名资料写进稿件。
6. 设计稿与后续正式功能实现分别评审；所有仓库改动仍走 PR，遵循根目录 AGENTS.md。

## 索引

| 日期 | 主题 | 版本 | 状态 | 文件 | 关联 |
| --- | --- | --- | --- | --- | --- |
| 2026-11-14 | 首次连接引导融入连接状态 | v02 | 已实现，待评审 | [说明与源稿](2026-11-13-first-connection/v02/SPEC.md) · [浅色验证](2026-11-13-first-connection/v02/previews/verify-light.png) · [深色重试](2026-11-13-first-connection/v02/previews/retry-dark.png) | #484 |
| 2026-11-13 | 首次连接引导 | v01 | 由 v02 替代，保留历史 | [说明与源稿](2026-11-13-first-connection/v01/SPEC.md) · [浅色导入](2026-11-13-first-connection/v01/previews/import-light.jpg) · [深色接管](2026-11-13-first-connection/v01/previews/takeover-dark.jpg) | #467 |
| 2026-11-10 | 节点卡片随高度分页 | v01 | 已实现，待评审 | [说明与源稿](2026-11-10-adaptive-node-pagination/v01/SPEC.md) · [三行](2026-11-10-adaptive-node-pagination/v01/previews/three-rows-light.jpg) · [最小窗口两行](2026-11-10-adaptive-node-pagination/v01/previews/two-rows-dark.jpg) | #410 |
| 2026-11-06 | 顶栏更新入口：概览不再占高 | v01 | 已实现，待评审 | [说明](2026-11-06-toolbar-update/v01/SPEC.md) · [交互矩阵](2026-11-06-toolbar-update/v01/matrix.html) · [浅色](2026-11-06-toolbar-update/v01/previews/ready-zh-light.jpg) · [窄窗深色](2026-11-06-toolbar-update/v01/previews/compact-en-dark.jpg) | #337 |
| 2026-11-06 | 概览左侧图表与段间距同步伸缩 | v03 | 已实现，视觉验收待补 | [说明](2026-11-05-overview-height/v03/SPEC.md) · [尺寸矩阵](2026-11-05-overview-height/v03/matrix.html) · [源稿](2026-11-05-overview-height/v03/source.zip) | #318 |
| 2026-11-05 | 概览左侧：最小间距、渐进留白与图表上限 | v02 | 由 v03 替代 | [说明/运行](2026-11-05-overview-height/v02/SPEC.md) · [生产组件矩阵](2026-11-05-overview-height/v02/matrix.html) · [源稿](2026-11-05-overview-height/v02/source.zip) · [检查](2026-11-05-overview-height/v02/checks.json) | #307 |
| 2026-11-05 | B1 主按钮配色：明暗自适应与固定浅底图标 | v02 | 用户已确认，已实现待 PR 评审 | [说明](2026-11-05-button-logo/v02/SPEC.md) · [正式资源预览](2026-11-05-button-logo/v02/previews/production-assets.png) · [源稿](2026-11-05-button-logo/v02/source.zip) · [校验](2026-11-05-button-logo/v02/checks.json) | #301 / PR #302 |
| 2026-11-05 | 主按钮同色 Logo：明暗主题与原生 Dock 预览 | v01 | 自适应方向已确认，实现见 v02 | [说明/运行](2026-11-05-button-logo/v01/SPEC.md) · [配色对比](2026-11-05-button-logo/v01/previews/comparison.png) · [源稿](2026-11-05-button-logo/v01/source.zip) · [原生校验](2026-11-05-button-logo/v01/previews/native-checks.txt) | #301 |
| 2026-11-05 | B1 / A 岩蓝正式品牌资源替换 | v02 | 已实现；后续配色由 #301 替代 | [说明](2026-11-05-logo-palette/v02/SPEC.md) · [正式资源预览](2026-11-05-logo-palette/v02/previews/production-assets.png) · [源稿](2026-11-05-logo-palette/v02/source.zip) · [校验](2026-11-05-logo-palette/v02/checks.json) | #297 / PR #298 |
| 2026-11-05 | B1 Logo 配色：六个低饱和灰蓝提案 | v01 | A 已确认；正式资源见 v02 | [规格/运行](2026-11-05-logo-palette/v01/SPEC.md) · [六色预览](2026-11-05-logo-palette/v01/previews/comparison.png) · [现用与 A](2026-11-05-logo-palette/v01/previews/recommended.png) · [源稿](2026-11-05-logo-palette/v01/source.zip) | #297 |
| 2026-11-05 | 概览左侧：有限留白与图表优先的高度分配 | v01 | 被 v02 替代，保留追溯 | [说明/运行](2026-11-05-overview-height/v01/SPEC.md) · [源稿](2026-11-05-overview-height/v01/source.zip) · [矩阵入口](2026-11-05-overview-height/v01/matrix.html) · [检查](2026-11-05-overview-height/v01/checks.json) | #295 |
| 2026-11-05 | 连接详情行居中及 PR CI 故障分析 | v02 | 已实现，实际视觉验收待补 | [说明](2026-11-04-ui-polish/v02/SPEC.md) · [源稿](2026-11-04-ui-polish/v02/source.zip) · [检查](2026-11-04-ui-polish/v02/checks.json) | #291 / PR #292 |
| 2026-11-04 | 设置焦点、日志悬停与多选筛选、页面留白 | v01 | 已实现，工具受限，实际视觉验收待补 | [规格/预览运行](2026-11-04-ui-polish/v01/SPEC.md) · [源稿](2026-11-04-ui-polish/v01/source.zip) · [验证边界](2026-11-04-ui-polish/v01/checks.json) | #291 |
| 2026-11-04 | 端口读值/显式编辑、节点只留卡片、概览统一表面 | v01 | 已实现，实际截图待浏览器可用后补验 | [规格/运行方式](2026-11-04-ui-simplification/v01/SPEC.md) · [交互原型](2026-11-04-ui-simplification/v01/index.html) · [源稿](2026-11-04-ui-simplification/v01/source.zip) · [验证](2026-11-04-ui-simplification/v01/checks.json) | #285 |
| 2026-11-04 | 桌面九项验收修复及卡片底部/滚动间距等后续调整 | v02 | 已实现，待 PR 评审 | [规格](2026-11-04-acceptance/v02/SPEC.md) · [卡片底部](2026-11-04-acceptance/v02/previews/cards-bottom-1120.png) · [日志](2026-11-04-acceptance/v02/previews/logs-light-1120.png) · [覆写](2026-11-04-acceptance/v02/previews/overrides-light-1120.png) · [源稿](2026-11-04-acceptance/v02/source.zip) · [验证边界](2026-11-04-acceptance/v02/checks.json) | #279 |
| 2026-11-04 | 桌面九项验收修复首轮 | v01 | 已被 v02 后续调整替代，留存追溯 | [规格](2026-11-04-acceptance/v01/SPEC.md) · [源稿](2026-11-04-acceptance/v01/source.zip) | #279 |
| 2026-11-04 | 桌面十一项界面整理、节点密度与共享控件 | v01 | 已实现，待 PR 评审 | [规格](2026-11-04-layout/v01/SPEC.md) · [概览](2026-11-04-layout/v01/previews/overview-light-1440.png) · [节点](2026-11-04-layout/v01/previews/nodes-list-light-1120.png) · [源稿](2026-11-04-layout/v01/source.zip) | #275 / 前置 #276 |
| 2026-11-03 | 诊断、连接、日志、订阅与恢复工作流 | v01 | 已实现，待 PR 评审 | [规格](2026-11-03-workflows/v01/SPEC.md) · [诊断](2026-11-03-workflows/v01/previews/diagnosis.jpg) · [恢复](2026-11-03-workflows/v01/previews/recovery-confirm.jpg) · [源稿](2026-11-03-workflows/v01/source.zip) | #274 |
| 2026-11-03 | 桌面基础交互九项修复与连接状态卡片 | v01 | 已实现，待 PR 评审 | [规格](2026-11-03-ui-reliability/v01/SPEC.md) · [浅色](2026-11-03-ui-reliability/v01/previews/overview-light.jpg) · [深色英文](2026-11-03-ui-reliability/v01/previews/overview-dark-880.jpg) · [源稿](2026-11-03-ui-reliability/v01/source.zip) | #271 |
| 2026-11-02 | 官网 V2：紧凑导航、App 明暗主题与中英文 | v02 | 已实现，待 PR 评审 | [规格](2026-11-02-website-polish/v02/SPEC.md) · [浅色](2026-11-02-website-polish/v02/previews/home-zh-light.jpg) · [深色英文](2026-11-02-website-polish/v02/previews/home-en-dark.jpg) · [源稿/动效](2026-11-02-website-polish/v02/source.zip) | #261 / #260 |
| 2026-11-02 | 官网首屏对齐、导航与 Linux 社区支持说明 | v01 | 已实现，待 PR 评审 | [规格](2026-11-02-website-polish/v01/SPEC.md) · [首屏](2026-11-02-website-polish/v01/previews/home-1440.jpg) · [手机](2026-11-02-website-polish/v01/previews/home-390.jpg) · [源稿/动效](2026-11-02-website-polish/v01/source.zip) | #259 |
| 2026-10-31 | Dock Logo 色彩一致性：原生加载与 ICC 导出修复 | v01 | 已实现，原生验色与独立程序实际 Dock 验收通过 | [规格](2026-10-31-dock-color/v01/SPEC.md) · [原生对比](2026-10-31-dock-color/v01/previews/native-comparison.png) · [验色窗口](2026-10-31-dock-color/v01/previews/smoke-window.jpg) · [源快照](2026-10-31-dock-color/v01/source.zip) | #241 |
| 2026-10-31 | 公共按钮：两种动作风格、顶部/内容区与全桌面迁移 | v01 | 已实现，待 PR 评审 | [规格](2026-10-31-buttons/v01/SPEC.md) · [状态稿](2026-10-31-buttons/v01/previews/gallery-light.jpg) · [节点回归](2026-10-31-buttons/v01/previews/production-nodes.jpg) · [源快照](2026-10-31-buttons/v01/source.zip) | #231 |
| 2026-10-31 | 公共选择器：统一样式、键盘与浮层过渡 | v04 | 已实现，待 PR 评审 | [规格](2026-10-30-node-toolbar/v04/SPEC.md) · [生产预览](2026-10-30-node-toolbar/v04/previews/production-open.jpg) · [源快照](2026-10-30-node-toolbar/v04/source.zip) | #227 |
| 2026-10-30 | 连接路径：信息组与传输轨迹居中 | v01 | 已实现，待 PR 评审 | [规格](2026-10-30-route-centering/v01/SPEC.md) · [预览](2026-10-30-route-centering/v01/previews/regular.jpg) | #227 |
| 2026-10-30 | 节点搜索与筛选行：单层焦点、320px限宽 | v03 | 已实现，待 PR 评审 | [规格](2026-10-30-node-toolbar/v03/SPEC.md) · [实际预览](2026-10-30-node-toolbar/v03/previews/wide.jpg) · [源快照](2026-10-30-node-toolbar/v03/source.zip) | #227 |
| 2026-10-30 | B1 主色 B：正式品牌资产与主题一致性 | v02 | 已实现，待 PR 评审 | [规格](2026-10-29-logo-color/v02/SPEC.md) · [正式效果](2026-10-29-logo-color/v02/previews/production-1120.jpg) · [源快照](2026-10-29-logo-color/v02/source.zip) | #202 / #204 |
| 2026-10-29 | B1 主色微调：更深、更鲜明的靛蓝 | v01 | B 色已确认，正式实现见 v02 | [交互稿](2026-10-29-logo-color/v01/index.html) · [规格](2026-10-29-logo-color/v01/SPEC.md) · [对比预览](2026-10-29-logo-color/v01/previews/comparison-1120.jpg) | #202 |
| 2026-10-29 | 通知中心与操作反馈降噪：正式实现 | v02 | 已实现，待 PR 评审 | [规格/运行](2026-10-29-notifications/v02/SPEC.md) · [源快照](2026-10-29-notifications/v02/source.zip) · [预览](2026-10-29-notifications/v02/previews/center-light.jpg) · [验证](2026-10-29-notifications/v02/previews/checks.json) | #196 |
| 2026-10-29 | 通知中心与分级降噪策略 | v01 | 已确认，正式实现见 v02 | [原型](2026-10-29-notifications/v01/index.html) · [规格](2026-10-29-notifications/v01/SPEC.md) · [策略矩阵](2026-10-29-notifications/v01/POLICY.md) · [预览](2026-10-29-notifications/v01/previews/center-light.jpg) | #196 |
| 2026-10-29 | 更新提示信息条：统一32px高度 | v04 | 已实现，待 PR 合并 | [规格](2026-10-29-overview-sidebar/v04/SPEC.md) · [源快照](2026-10-29-overview-sidebar/v04/source.zip) · [预览](2026-10-29-overview-sidebar/v04/previews/1120x734-ready.jpg) | #189 / #190 |
| 2026-10-29 | 概览 V2 正式实现：连接路径收边与卡片高度上限 | v03 | 已实现，待 PR 合并 | [规格/复现方式](2026-10-29-overview-sidebar/v03/SPEC.md) · [源文件快照](2026-10-29-overview-sidebar/v03/source.zip) · [预览](2026-10-29-overview-sidebar/v03/previews/1120x734-update.jpg) | #189 |
| 2026-10-29 | 概览右侧多尺寸适配 | v01 | 右侧方向已认可 / 未应用正式代码 | [原型](2026-10-29-overview-sidebar/v01/index.html) · [规格](2026-10-29-overview-sidebar/v01/SPEC.md) · [预览](2026-10-29-overview-sidebar/v01/previews/1120x734.jpg) | #189 |
| 2026-10-29 | 概览整体布局：状态、连接路径与趋势 | v02 | 已确认，后续实现见 v03 | [原型](2026-10-29-overview-sidebar/v02/index.html) · [规格](2026-10-29-overview-sidebar/v02/SPEC.md) · [预览](2026-10-29-overview-sidebar/v02/previews/1120x734.jpg) | #189 |
| 2026-10-28 | Logo 三方向六提案 | v01 | B1 已确认 | [规格](2026-10-28-logo/v01/SPEC.md) · [预览](2026-10-28-logo/v01/previews/contact-sheet.png) | #178 |
| 2026-10-28 | B1 六种深化 | v02 | 留存，未采用 | [规格](2026-10-28-logo/v02/SPEC.md) · [预览](2026-10-28-logo/v02/previews/contact-sheet.png) | #178 |
| 2026-10-28 | B1 原版正式品牌资产 | v03 | 已确认 / 已实现 | [规格](2026-10-28-logo/v03/SPEC.md) · [预览](2026-10-28-logo/v03/previews/brand-sheet.png) | #178 |
| 2026-10-28 | 官网 V3 白色首屏 + 原功能/上手/下载卡片 | v01 | 已实现，待评审 | [规格](2026-10-28-website-implementation/v01/SPEC.md) · [源文件快照](2026-10-28-website-implementation/v01/source.zip) · [首屏预览](2026-10-28-website-implementation/v01/previews/home-1440.jpg) | #176 |
| 2026-10-27 | 概览连接路径数据传输动效 | v01 | 提案 | [原型](2026-10-27-connection-flow/v01/index.html) · [规格](2026-10-27-connection-flow/v01/SPEC.md) | #160 |
| 2026-10-28 | 概览连接路径单条快速传输 | v02 | 提案 | [原型](2026-10-27-connection-flow/v02/index.html) · [规格](2026-10-27-connection-flow/v02/SPEC.md) | #160 |
| 2026-10-28 | 概览连接路径单条传输 / 间隔 2.56 秒 | v03 | 提案 | [原型](2026-10-27-connection-flow/v03/index.html) · [规格](2026-10-27-connection-flow/v03/SPEC.md) | #160 |

| 2026-10-29 | 日志筛选工具栏 | v01 | 已实现 / 待确认 | [规格](2026-10-29-log-toolbar/v01/SPEC.md) · [浅色预览](2026-10-29-log-toolbar/v01/previews/light.png) · [深色预览](2026-10-29-log-toolbar/v01/previews/dark.png) | #193 |

| 2026-10-30 | 节点工具栏降噪 | v01 | 整体方向已确认 | [原型](2026-10-30-node-toolbar/v01/index.html) · [规格](2026-10-30-node-toolbar/v01/SPEC.md) | #207 |
| 2026-10-30 | 节点工具栏与可用滑动开关 | v02 | 已实现 | [规格](2026-10-30-node-toolbar/v02/SPEC.md) · [实际浅色](2026-10-30-node-toolbar/v02/previews/light.png) · [实际深色](2026-10-30-node-toolbar/v02/previews/dark.png) | #207 |

| 2026-10-31 | Dock 图标安全区：恢复与系统 App 一致的视觉尺寸 | v02 | 已实现，原生验证通过，待安装验收 | [规格](2026-10-31-dock-color/v02/SPEC.md) · [原生对比](2026-10-31-dock-color/v02/previews/native-comparison.png) · [源快照](2026-10-31-dock-color/v02/source.zip) | #246 |
