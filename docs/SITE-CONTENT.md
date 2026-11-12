# 官网内容覆盖与优化清单

审查日期：2026-11-05。关联 #313。基线：main `a2063c1dfee9992dc35393fb1c42081a746b6113`（v0.13.5）。
本轮补充已有功能说明，保持用户确认的白色/深色主题、简洁首屏和非截图式连接示意。

## 本轮补充与纠正

| 官网内容 | 实际实现依据 | 呈现位置 |
| --- | --- | --- |
| 订阅的上次成功、下次更新/重试、失败原因和新增/移除/变化节点数量 | `apps/desktop/src/pages/Profiles/index.tsx` 的订阅状态；`apps/desktop/src-tauri/src/workflow.rs` 的节点差异统计 | 首页订阅摘要、功能详解、指南订阅更新 |
| 自动选择组手动固定与恢复自动 | `apps/desktop/src/pages/Proxies/index.tsx` 的 automaticGroup、restoreAutomatic | 功能详解节点部分 |
| 逐项诊断进度、通过/失败/跳过、首个阻塞项与后续操作 | `apps/desktop/src/pages/Diagnostics/index.tsx` 的 DIAG_STEPS 和 first blocker | 首页诊断摘要、功能详解、排障指南 |
| 完整连接详情、逐字段/整体复制、搜索排序与暂停刷新 | `apps/desktop/src/pages/Connections/index.tsx` 的 connectionDetails 和 Dialog | 功能详解观测、排障指南 |
| 日志级别多选、关键词、暂停跟随/返回底部、单条复制与当前筛选结果脱敏导出 | `apps/desktop/src/pages/Logs/index.tsx`；`apps/desktop/src-tauri/src/workflow.rs` 的 export_log_lines | 功能详解观测、排障指南 |
| 站点测试时间、出口、链路变化和 HTTP 限制响应的含义 | `apps/desktop/src/components/SiteConnectivity/index.tsx` 的 testedAt、outbound、pathChanged | 功能详解观测、排障指南 |
| 配置恢复入口、快照时间和数量、恢复范围及重启影响 | `apps/desktop/src/components/ConfigRecovery/index.tsx`；`crates/crossbow-core/src/store.rs`；`workflow.rs` 的 restore_config_snapshot | 新增功能详解恢复部分与指南恢复流程 |
| 手动检查只检查版本，下载需确认；自动下载仅用于定时检查 | `apps/desktop/src-tauri/src/lib.rs` 的 update_check_now 使用 check_once(false, false)；`update_check.rs` 的定时任务；`pages/Settings/index.tsx` |  新增功能详解更新部分与指南更新流程 |
| Windows 安装器附带 TUN 服务与文件占用/服务修复说明入口 | `docs/WINDOWS-INSTALL.md`；`apps/desktop/src-tauri/windows` | 指南 Windows 部分及仓库文档链接 |
| 顶部直接切换配置、应用外观/语言、局域网代理共享 | `components/ProfileSwitcher/index.tsx`；`pages/Settings/index.tsx` 的 theme/lang/allow-lan | 功能详解及端口指南 |
| 端口需显式修改/应用，Enter 应用、Escape 取消，失焦不保存 | `apps/desktop/src/pages/Settings/index.tsx` 的端口编辑流程 | 指南端口部分 |

纠正内容：移除下载区与当前版本脱节的“岩蓝 Logo”摘要；版本与资产继续来自发布 API。删除“诊断永久持久化”“所有失败一键修复”的描述，改为当前会话内结果和可用修复操作；更新“代理页/覆写组”等旧页面名称。

Linux 社区贡献者、x86_64 归档、systemd/polkit TUN 和当前验收边界在本轮前已出现在官网，不重复新增。iOS 仍仅说明开发验证中、尚未公开分发，不能把工程骨架当作可下载产品。

## 后续优化建议（本 PR 未实施）

优先级是建议顺序。下列视觉体验判断基于当前源码，浏览器连接超时，需在实施时用真实窗口确认。

下表 P1 五项已于 2026-11-07 实施（PR：官网 P1 优化，Ref #349）：分组目录与语义锚点、scroll-margin-top 锚点定位、下载状态机（加载/失败/无资产 + 重试 + localStorage 缓存 + 系统推荐）、平台支持矩阵（/features#support-matrix）、快速开始章节（/guide#quick-start）。P2/P3 仍待实施。

| 优先级 | 当前情况与依据 | 建议 |
| --- | --- | --- |
| P1 | 指南缺少目录；功能页目录是按钮，目标用 feature-序号命名，插入功能就会改变编号 | 用稳定的语义锚点、可复制链接和分组目录，长页增加当前章节提示；指南按上手/排障/高级配置分组 |
| P1 | 头部 sticky，但 guide-card 的 scroll-margin-top 为 0，目录点击直接 scrollIntoView | 为所有章节预留导航高度，避免定位后标题被导航遮挡；同时检查手机菜单关闭后的位置 |
| P1 | useLatestRelease 将请求失败、加载中和缺少资产均表现为 null，没有独立提示/重试 | 下载区区分加载、网络失败、该平台无资产；增加重试与缓存，保留发布页兜底。按系统推荐下载项，同时保留其他平台入口 |
| P1 | 平台边界分散在 FAQ、功能页和 Linux 指南中 | 建立 macOS/Windows/Linux/iOS 支持矩阵，明确 CPU、系统代理、TUN、内核、自动更新和实验状态 |
| P1 | 首页上手入口跳入指南，而指南从双内核与 JS 开始 | 提供安装→导入→选节点→开启接管→验证的快速开始路径；进阶内容与首次使用分开，按平台给步骤 |
| ~~P2~~ 已实施 | （原）功能页重复“概要+多条说明”的相同双栏；指南全部堆叠同一种卡片 | 按管理配置、连接接管、观测排障、维护恢复归类；减少重复摘要，用简洁流程图表达跨页面操作，延续非截图式展示 |
| ~~P2~~ 已实施 | （原）FAQ 主要是协议、平台和 macOS 安全提示 | 补常见故障导航：运行中但浏览器不通、更新订阅失败、Windows 服务缺失；链接到对应诊断/指南章节 |
| ~~P2~~ 已实施 | （原）Code 组件只有 pre/code；示例会过滤 proxies，但没有同步清理原策略组引用，且假定存在“自动选择” | 示例增加复制与反馈，提供前置条件和可验证完整配置；用实际覆写校验覆盖示例，避免直接套用后出现悬空引用 |
| ~~P2~~ 已实施 | （原）HashRouter 用 #/features 等片段切页；语言仅保存在本地，index.html 无页面独立 canonical/OG | **已迁移 Astro 静态输出（#363）**：四页 × 中英构建期预渲染为完整 HTML；BrowserRouter 净 URL；per-page title/description/canonical/OG + hreflang 互指 + JSON-LD；sitemap + robots；语言走 /en/* 路由（偏好客户端仍可覆盖）。OG 分享图为后续项 |
| P2 | 首屏主要使用 svh 最小高度，但内容间距固定，未按矮窗口高度压缩 | 复核 390×568、横屏和短桌面窗口；确保下载按钮容易找到，不通过压缩连接节点文字来腾空间 |
| P3 | 导航同时呈现 GitHub、外观、语言、下载及手机菜单 | 实测 360/390/768/1000px 的中英文；可将低频偏好收进一个入口，保证下载和主导航仍清楚，避免仅为变化再次重画导航 |
| ~~P3~~ 已实施（字体部分） | （原）构建含两份 TTF 合计约 193KB；所有页面同包约 281KB JS（gzip 约 96KB）。Astro 迁移后页面 JS 已近乎清零（仅下载岛），字体 TTF 193KB → WOFF2 58KB | 有实际加载数据后再评估 WOFF2、路由拆包和按需加载；首页轻量为目标，不增加装饰性大图 |

搜索可见性依据：[Google Search Central 的 JavaScript SEO 指南](https://developers.google.com/search/docs/crawling-indexing/javascript/javascript-seo-basics)建议用 History API 而不是片段加载不同页面，并说明预渲染、页面独立标题与描述的作用。是否迁移是部署与产品选择，不是本轮直接修改路由。

## 2026-11-08 本轮实施（P2 收尾，Ref #365）

- 功能页 14 章节按「管理配置 / 连接接管 / 观测排障 / 维护恢复」四组归类，组头分隔 + 分组目录；语义锚点 id 全部不变
- FAQ 新增三条故障导航（运行中但浏览器不通 / 订阅更新失败 / Windows 服务缺失），分别链接诊断向导、订阅更新、Windows 安装说明
- JS 覆写示例补前置条件（节点名过滤、策略组悬空引用）与代码复制按钮
- 字体 TTF → WOFF2（193KB → 58KB），@font-face 保留 TTF 回退
- 矮窗口与导航密度实测见 #365 验收记录

## 内容持续维护

发版时除版本号外，按“新增能力→实际入口→平台限制→中英文→功能/指南/FAQ”核对。不要把每次修复都增加为首页卡片；首页保留概览，细节进入功能和指南。避免在会随版本号自动更新的下载模块中放需要人工维护的版本宣传文案。

## 本轮验证

- 官网 TypeScript/Vite 构建通过。
- 原有五项偏好测试通过：系统/保存的外观、语言回退、存储受限时启动。
- 使用真实 React 页面、PreferencesProvider 和 MemoryRouter，在 Node 中完成首页/功能/指南/下载四页的中文和英文静态内容渲染：单主标题、新功能文案、指南锚点、英文功能页无中文串及过时版本摘要移除均通过。
- 静态内容渲染不执行浏览器布局、useEffect、滚动或真实发布 API，不能当作视觉或交互验收。
- 浏览器控制连接超时；实际尺寸、主题对比、锚点位置与下载网络失败交互待验。本 PR 没有修改上述交互。
- 未运行桌面/内核测试，因为未修改桌面行为。CI 按用户约定不持续轮询。
