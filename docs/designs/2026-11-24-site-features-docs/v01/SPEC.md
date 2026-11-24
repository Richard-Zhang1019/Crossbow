# #653 官网：功能详解并入文档工作区布局，内容区滚动条收细 · v01

日期：2026-11-24 · 关联 issue：#653 · 状态：已实现，待 PR 评审

## 问题

1. **内容区滚动条**：文档工作区让文章列自己滚动（`body.documentation` 固定视口），但 `.docs-article` 用浏览器默认滚动条 + `scrollbar-gutter:stable`。系统开启"始终显示滚动条"时，中栏右侧出现 15px 宽灰槽，压在中栏与本文目录之间。
2. **两页布局不一致**：`/guide` 已迁移到 docs shell（左：文档概览 + 筛选目录；中：文章；右：本文内容），而 `/features` 仍是旧的 `.page-grid + .rail`：整页滚动、左栏无筛选、无右栏目录、无移动端折叠。

## 方案

- **滚动条**：只改观感，保留"文章列滚动、页头与左栏常驻"的文档工作区行为。`.docs-article` 使用 `scrollbar-width: thin` + `scrollbar-color: 半透明前景/透明轨道`，WebKit 侧 `::-webkit-scrollbar` 11px、透明轨道、3px 内缩圆角滑块；实测槽宽 15px → 11px，轨道不再可见。
- **布局**：新增 `FeatureDocs.astro`，复用 `docs.css` 的 `.docs-shell / .docs-sidebar / .docs-article / .docs-onpage`，把功能数据抽到 `src/docs/features.ts`；`/features` 与 `/guide` 共享同一套三栏骨架：
  - 左栏：使用文档入口、筛选目录（复用 `data-doc-search` 约定 + 空状态）、按 支持矩阵/管理配置/连接接管/观测排障/维护恢复 分组；
  - 中栏：文档式标题区（面包屑 + 版本 meta）+ 正文 + 上一篇/下一篇 + 反馈/下载链接；
  - 右栏：本文内容（16 节，超高时独立滚动并高亮当前节）；
  - 移动端：`details` 折叠目录，默认收起。
- 锚点 id 全部保持（`/features#support-matrix` 等外链仍有效）；旧 rail 的进度条/scrollspy 脚本删除，替换为目录高亮 + 受控锚点。
- 布局一致性由 `Layout.astro` 把 `/features` 纳入 `body.documentation`（页脚与 /guide 一样隐藏）。

## 证据

- `scripts/dev/verify-site-features.mjs`（生产静态构建 + Chromium）：
  - docs shell 三栏、目录 5 组、本文目录 16 项、页面本身不滚动；
  - 文章列滚动条槽宽 ≤12px 且 `::-webkit-scrollbar` 显式为 11px；
  - 目录筛选命中/空状态、锚点跳转只滚文章列并保留 hash、本文目录点击滚动、当前节高亮唯一；
  - 英文路由同构；390px 下目录默认折叠。
- `verification/build.log`：`npm run build`（含 8 项站点测试）通过；`verification/verify.log` 通过。
- `verification/scrollbar.json`：before gutter 15px / after 11px（Chromium classic scrollbar 测量）。
- 预览：改前 `previews/before-features.png`、`before-mobile.png`；改后 `after-features-light.png`、`after-features-dark.png`、`after-features-mobile.png`、`after-features-scrolled.png`、`after-guide-article.png`。

## 边界与待验

- 无头截图不绘制滚动条本身，槽宽用几何测量 + CSS 计算值验证；系统"始终显示滚动条"下的真实观感需在浏览器/安装版复查。
- 未改 `/guide` 的内容与导航数据；未引入新的站内依赖。
- Windows/Linux 浏览器（Firefox `scrollbar-width: thin`、Edge Chromium）未逐一实机验收。
