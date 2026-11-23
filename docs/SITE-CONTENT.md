# 官网内容覆盖与维护

## 当前文档标准（2026-11-23，#651）

基线为 main `0267e60004846bbf54fa3f97c304449f7d8fa0ce`，核对 v0.15.0 至已发布 v0.15.3 的代码。本节替代旧版单页指南的组织方式；下方历史记录保留追溯。

### 信息架构与视觉规范

- `/guide`、`/zh/guide` 是文档概览；31 个功能各有中英独立静态路由文件，16 个原章节保留，新增15篇功能说明。全站构建72页，其中功能文档62页，加双语概览共64个文档页面。
- 导航源在 `apps/site/src/docs/navigation.ts`，页面源为 `src/pages/guide/` / `src/pages/zh/guide/`；旧内容按功能拆到 `components/docs-content/`，不是继续渲染一个大文件并隐藏无关章节。
- 桌面是常驻目录 + 独立正文滚动 + 宽屏本文目录，保留品牌、Manrope/中文系统字体和明暗色阶；文档标题32px、分组22px、小节17px、正文15px、目录13px、辅助12px，消费文档语义/刻度变量，保持文章行宽约740px。
- 800px及以下目录为原生 details 披露控件，可筛选目录、Escape关闭，选中路由后收起；输入16px避免手机聚焦放大。代码/表格溢出只在自身区域处理。
- 真实路由支持直接访问、刷新、前进/后退、语言对应和独立 title/description/canonical/hreflang/sitemap。旧 `/guide#章节` 自动转对应路由，无JS提供手动旧章节索引；未知锚点不任意跳转。
- 目录筛选仅筛标题/简介，不冒充全文搜索。没有引入SPA/客户端Markdown解析依赖；无JavaScript时仍提供内容和原生目录导航。

### 新补内容与实现依据

| 文档路由 | 核对的实际实现 | 重要边界 |
| --- | --- | --- |
| local-import | `pages/Profiles/ImportCompatibility.tsx`；`src-tauri/src/commands/profiles.rs`；core `subscription.rs` | 本地解析、整批预检、TUIC/REALITY跨内核限制；导入不等于切换 |
| nodes | `pages/Proxies/index.tsx` | 选择与测速分开；自动组手动固定/恢复 |
| custom-groups | `components/CustomGroups` | 当前来源内联具体节点；创建不自动改变出口；自动测试会联网 |
| user-rules | `components/UserRules`；core `user_rules.rs` | 进程→用户规则→规则集→订阅；仅规则模式；失效目标不降级DIRECT |
| process-rules | `components/ProcessRules` | 使用实际进程身份；未知进程不可猜测；失效目标暂不生效 |
| rule-sets | `components/RuleSets`；core `rule_sets.rs` | 文本子集、缓存与刷新；2MiB/50000条，不支持MRS/SRS |
| dns | `pages/Settings/DnsSettings.tsx` | 沿用/自定义；mihomo多上游与sing-box单上游/引导限制 |
| network-scenes | `components/SceneRules` | SSID与macOS权限；进入/离开恢复偏好，不额外开启TUN |
| lan-sharing | `pages/Settings/LanSharing.tsx` | 地址/混合端口、网络隔离/防火墙；不对外网作连通保证 |
| connections | `pages/Connections/index.tsx` | 真实命中/最后快照/未知进程；规则入口 |
| statistics | `pages/Stats` | 本机快照估算、30天/容量裁剪、暂停/清除，不作为账单 |
| node-quality | `pages/Proxies/NodeQuality.tsx` | 按目标统计、手动采样、控制器错误排除，不参与自动选路 |
| node-tests | `components/NodeProbes` | 隔离测试，30秒，4MiB下载/1MiB上传，非带宽上限 |
| dns-diagnostics | `pages/Diagnostics/DnsCheck.tsx` | A/AAAA和上下文、未观测不等于失败、不臆测上游/缓存 |
| migration | core `config_package.rs`；`docs/config-package/README.md` | v2未加密且8MiB；不含域名/IP用户规则、自定义组、机器接管状态 |

原章节补正：恢复入口改为「配置来源 → 备份与迁移 → 恢复快照」；原脚本示例改为保留规则、同步清理已移除节点的组引用，并增加可执行例子回归。快照恢复与配置包范围不同，不承诺完整应用备份。

### 后续添加或修改文档

1. 在导航源登记 slug、组别、双语标题/简介；在两种语言的 pages 目录新增同名 `.md` 或 `.astro`，每功能每语言仅一个路由。使用公共 DocsLayout。
2. 内容按「入口→步骤→结果解释→支持边界→失败后下一步」书写，对照当前生产实现。Markdown内跨文档链接使用语言无关 `/guide/slug`，构建插件补语言和 SITE_BASE。
3. 功能行为变化时同步两种语言、相关互链与覆盖表。新旧协议支持要区分本客户端导入/转换路径和上游内核能力；安全/隐私范围不能靠推测扩大。
4. `npm run build --prefix apps/site` 同时执行网站单元回归；`node scripts/dev/verify-site-docs.mjs` 验证实际静态页面及交互。子路径验收需以同一 SITE_BASE 构建并运行脚本。
5. 设计变化另存版本化源稿、说明、manifest和实际预览；本次见 `docs/designs/2026-11-23-site-docs/v01/`。不要把旧长页截图当成新路由效果。

## 历史记录：v0.13 系列官网补充


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
