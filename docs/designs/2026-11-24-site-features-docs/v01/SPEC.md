# #653 官网：功能详解按文档路由拆分，并入文档工作区布局 · v01

日期：2026-11-24 · 关联 issue：#653 · 状态：已实现，待 PR 评审

## 问题

1. **内容区滚动条**：文档工作区让文章列自己滚动（`body.documentation` 固定视口），但 `.docs-article` 用浏览器默认滚动条 + `scrollbar-gutter:stable`。系统开启"始终显示滚动条"时，中栏右侧出现 15px 宽灰槽，压在中栏与本文目录之间。
2. **两页结构不一致**：`/guide` 每个左侧目录项都有独立路由文件（`/guide/<slug>`），而 `/features` 是单页锚点滚动（旧 `.page-grid + .rail`），左栏无筛选、无右栏目录、无移动端折叠。

## 方案

- **滚动条**：只改观感，保留"文章列滚动、页头与左栏常驻"的文档工作区行为。`.docs-article` 使用 `scrollbar-width: thin` + `scrollbar-color: 半透明前景/透明轨道`，WebKit 侧 11px、透明轨道、3px 内缩圆角滑块；实测槽宽 **15px → 11px**，轨道不可见。
- **路由拆分**：功能页改为与使用文档同构的文档集：
  - `src/docs/feature-navigation.ts`：16 节 `{slug, group, title, description}`，5 个分组（总览/管理配置/连接接管/观测排障/维护恢复），`featureHref` 与 `legacySections`；
  - `DocsLayout.astro` 增加 `nav="features"`，与 `/guide` 复用同一套左栏/文章/本文目录/筛选/移动端折叠/分页逻辑；
  - 路由：`/features`（总览，5 组 16 个链接）、`/features/<slug>`（16 节），中英文各一套（共 34 个页面）；
  - 旧的 `/features#<slug>` 由总览页脚本迁移到 `/features/<slug>`（沿用 `/guide` 的 legacy hash 方案），站内三处 `/features#...` 链接同步改为路由。
  - 每节正文：标题/摘要来自导航数据，"能力与边界"列表或平台矩阵（矩阵说明单独一段）；右侧本文内容列出该节的 h2，底部上一篇/下一篇按目录顺序。
- `/features` 纳入 `body.documentation`（含子路由），页脚行为与 `/guide` 一致。

## 证据

- `scripts/dev/verify-site-features.mjs`（生产静态构建 + Chromium）：
  - 总览：目录 16 个链接且全部是 `/features/<slug>` 路由、筛选命中与空状态、页面本身不滚动；
  - 单节：独立 `h1`、当前目录项 `aria-current`、本文内容列出"能力与边界"、分页前进/后退、矩阵页渲染表格；
  - 旧锚点 `/features#diagnostics` 自动迁移到 `/features/diagnostics`；
  - 32 个路由（16 节 × 2 语言）HTTP 200、单 `h1`、共享 docs shell、canonical/hreflang、站内链接 0 404；
  - 文章列滚动条槽宽 ≤12px 且 `::-webkit-scrollbar` 显式 11px；
  - 深色主题截图、390px 目录默认折叠。
- `verification/build.log`：`npm run build`（含 8 项站点测试）通过；`verification/verify-features.log`、`verify-guide.log` 均通过。
- `verification/scrollbar.json`：before gutter 15px / after 11px（Chromium classic scrollbar 测量）。

## 预览

- 改前：`previews/before-features.png`（旧单页 rail）
- 改后：`previews/after-overview-light.png`、`after-overview-dark.png`、`after-section-light.png`、`after-section-dark.png`、`after-mobile.png`


## 评审调整（2026-11-24）

- **Header**：桌面 `.nav-shell` 80 → 68px（`--header-h`），hero 高度与锚点预留同步使用该变量；文档工作区顶部留白 32 → 24px，内容起点上移约 20px。
- **目录搜索框**：内边距 7/9 → 4/8，字号 13 → 12，图标 16 → 14（实测高度 26.8px）；focus 去掉 `outline`，改为“边框切 accent + 1px inset 阴影”，不再被目录滚动容器裁切，观感也更轻。
- **文章滚动条间距**：`.docs-article` 右内边距 12 → 40px（= 文档三栏的列间距 `--doc-gap`），正文到滚动条的距离与正文左侧到目录栏的距离一致；gutter 仍为 11px。
- 实测见 `verification/layout.json`：header 68、标题距 header 24、搜索框 26.8、focus 边框 accent 且无 outline、文章右内边距 40（列间距 40）、gutter 11。


## 评审调整（第三轮：宽度与总览入口）

- **文档工作区加宽**：文档页 `.shell` 上限 1240 → **1400px**；正文列上限 740 → **860px**；本文目录列 150 → **160px**。实测 1920/1440/1280 视口下 shell = 1400/1360/1200，正文 = 860/860/740。
- **总览入口降噪**：`/features` 顶部提示卡的大号主按钮改为右上角文字链接（`.docs-start-link`），卡片与说明保留，视觉重心回到正文。
- 验证脚本新增断言：正文列 ≥800px（1440 视口）、总览卡内不再出现主按钮。

## 评审调整（第四轮：目录间距与 header 对齐）

- **全站 shell 统一 1320px**（原 1240），文档页不再单独加宽；三栏 = 220 + 860 + 160 + 2×40，正好占满，**本文目录与正文之间只剩一个列间距（40px）**，不再有 1fr 余量造成的额外空白。
- **header 与内容同宽**：首页、功能详解、使用文档的 logo/产品名左边缘一致（1440 视口实测三者均为 60px）。
- 验证新增断言：目录与正文间距 36–44px、brand 与左侧目录栏左边缘差 ≤1px。

## 边界与待验

- 无头截图不绘制滚动条本身，槽宽用几何测量 + CSS 计算值验证；系统"始终显示滚动条"下的真实观感需在浏览器/安装版复查。
- 正文沿用原功能页文案与矩阵数据，未改写产品描述；`/guide` 的文档内容与导航未改动。
- Firefox/Edge/Windows 浏览器未逐一实机验收；旧外链若带其他 hash（如 `#foo`）不会被迁移，只处理 16 个已知节。
