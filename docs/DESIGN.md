# Crossbow 设计文档

> 跨平台（**macOS + Windows + iOS**）代理客户端，对标 Clash Verge 与 Mihomo Party，
> 目标是「Clash Verge 的易用性 + Surge 级别的可观测性与细节」。
>
> v3（2026-10）：重新纳入 Windows；仅不做 Linux。
> v2（2026-10）：平台范围曾收敛为 macOS + iOS。
> 状态：设计冻结，工程骨架搭建中。

---

## 1. 项目定位

**一句话定位**：开源的多内核（mihomo / sing-box）代理客户端，macOS / Windows 桌面端 + iOS 端共享同一套「大脑」。

**差异化的三个支点**（来自竞品痛点，见 §2）：

1. **稳**：升级不丢配置、TUN 开关不弹窗、任何退出路径都能还原系统状态——把竞品事故高发区做成强项。
2. **轻**：安装包小、待机内存低、支持「轻量模式」（GUI 退出后只留内核）。
3. **可观测**：连接、日志、流量、延迟可查可用，对齐 Surge 的仪表盘体验。

**平台策略**：macOS / Windows / iOS 三端，不做 Linux（明确记录不做）。桌面端同一 Tauri 壳覆盖双系统；iOS 因平台限制独立壳；三端共享 Rust 核心逻辑。

---

## 2. 竞品调研（调研于 2026-10，详见 §6 来源）

### 2.1 主流客户端概览

| 客户端 | 框架 / UI | 内核 | 平台 | 星数(约) | 状态 |
|---|---|---|---|---|---|
| Clash Verge Rev | Tauri 2 + React/TS | mihomo | Win/Mac/Linux | 90k+ | 活跃 |
| Mihomo Party（Clash Party） | Electron + React | mihomo | Win/Mac/Linux | 20k+ | 活跃（商业化争议后核心开发者另开 Sparkle 分支） |
| Sparkle | Electron（轻量改造） | mihomo | Win/Mac/Linux | 增长中 | 活跃 |
| FlClash | Flutter + Go FFI | mihomo | Win/Mac/Linux/Android | 31k+ | 活跃 |
| Hiddify (hiddify-app) | Flutter | sing-box | 全平台含 iOS | 33k+ | 活跃 |
| Stash | SwiftUI（自研内核） | 兼容 Clash | iOS/macOS | 闭源付费 | 活跃 |
| Shadowrocket | 原生 | 自研 | iOS | 闭源付费 | 活跃 |
| Surge | 原生（自研内核） | 自研 | macOS/iOS | 闭源付费 | 标杆 |
| Clash for Windows | Electron（闭源） | clashpremium | Win/Mac/Linux | — | 2023.11 删库停更 |

**背景**：2023 年 11 月 Clash for Windows 与原版 Clash 核心相继删库，社区迁移到 mihomo（原 Clash Meta）内核 + 各 GUI。mihomo 兼容 Clash YAML，是当前事实标准；sing-box 是另一极（JSON 配置、新协议跟进快、**libbox 移动端库成熟**）。

**与本项目最相关的两条产品线**：
- **桌面**：Clash Verge Rev / Mihomo Party——功能全但都是跨三平台折衷，无 Apple 深度整合（无 iCloud 同步、无 NE 集成）。
- **iOS/苹果系**：Shadowrocket / Stash / Loon / Surge——证明「苹果生态 + 订阅代理」需求真实且付费意愿高，但全部闭源付费。

### 2.2 各家优点（值得继承）

- **Clash Verge Rev**：安装包小、内存相对优秀；Profile（订阅档案）管理成熟；脚本 + Merge 双覆写机制；社区大、文档教程多。
- **Mihomo Party / Sparkle**：界面现代美观；**覆写（Override）能力最强**（YAML/JS 覆写、按订阅挂载）；WebDAV 备份；内置稳定/预览双内核通道；Sparkle 用 Unix Socket 而非 HTTP 控制内核（更安全更轻）。
- **FlClash**：交互简洁、全终端一致、配置多端共享；上手门槛最低。
- **Hiddify**：新协议（Reality/Hysteria2/TUIC）跟进快、订阅 URL 一键导入体验好。
- **Surge（设计标杆）**：仪表盘信息密度与响应速度、策略组一键并发基准测速、按规则/进程聚合的连接面板、步骤化排障思路。
- **Stash / Shadowrocket**：iOS 端「订阅导入即用」的零配置体验；按 App 代理；iOS 小组件/快捷指令集成。

### 2.3 缺点与痛点（设计输入，重点）

来自 GitHub issues、V2EX、Linux.do、Telegram 频道的真实用户反馈：

**稳定性类**
- 内存/CPU 失控：BT（uTorrent）大量连接把 mihomo 内存拉到 10GB+；Verge Rev 首页「当前代理」模块曾因重复刷新导致 CPU 飙高（2.2.3 修复）。
- Verge 功能多但设置项繁杂，新手不知道「服务模式/TUN/系统代理」三者的关系；「退出后断网」「升级丢配置」类事故反复出现。
- 连接面板大数据量卡顿；日志检索弱。

**苹果生态空缺**
- Verge 系无 iOS 端，配置无法跨设备；iOS 用户被迫用 Stash/Shadowrocket（付费、闭源、配置格式与桌面 Clash 生态有差异）。
- Verge 在 macOS 上 TUN 仍走 Linux 式 helper 思路，与 macOS 原生 NetworkExtension 生态（Surge 所用）脱节。

**生态风险类**
- mihomo 为 GPL-3.0，上游曾因「下游拒不遵守协议改名」公开发声评估停更——下游必须严谨遵守其许可与名称要求。
- Mihomo Party 商业化争议导致社区分裂（Sparkle 分叉），提示「社区信任」是此类项目的核心资产。

### 2.4 设计基准（借鉴清单）

| 设计点 | 来源 | 采纳 |
|---|---|---|
| 仪表盘（实时流量、当前出口、请求摘要一屏） | Surge | ✅ 首页即仪表盘 |
| 策略组一键并发基准测速、结果就地排序 | Surge | ✅ 代理页核心交互 |
| 连接面板（按规则/策略/进程聚合、单连接断开） | Surge | ✅ |
| Merge + Script 双覆写、按订阅挂载 | Verge Rev / Mihomo Party | ✅ |
| WebDAV 备份恢复 | Mihomo Party | ✅ + iCloud 快照 |
| 订阅导入即用（深链接/剪贴板识别） | Shadowrocket | ✅ |
| iCloud 跨设备配置同步 | 竞品均无（Apple 生态独有机会） | ✅ 差异化 |
| 步骤化网络诊断向导 | 自研（竞品均无） | ✅ |
| MitM 抓包、按需规则等 Surge 高级能力 | Surge | ⏸ 远期评估 |

---

## 3. 技术选型

### 3.1 总体架构：一套大脑，三种壳

```
┌────────────────────────────┐  ┌────────────────────────────┐  ┌──────────────────────────┐
│  macOS 桌面端               │  │  Windows 桌面端             │  │  iOS 端                   │
│  Tauri 2 (React + TS UI)   │  │  Tauri 2 (React + TS UI)   │  │  SwiftUI（原生 UI）        │
│         │ Tauri IPC        │  │         │ Tauri IPC        │  │         │                 │
│  crossbow-core (Rust) ◄────┼──┼──► crossbow-core (Rust) ◄───┼──┼──► crossbow-core (Rust)  │
└─────────┬──────────────────┘  └─────────┬──────────────────┘  └─────────┬────────────────┘
          │ sidecar 子进程                 │ sidecar 子进程                 │ 进程内 (libbox)
          ▼                               ▼                               ▼
┌──────────────────┐  普通模式    ┌──────────────────┐  普通模式   ┌────────────────────────────┐
│ mihomo / sing-box │            │ mihomo / sing-box │           │ NetworkExtension            │
│  (GUI 直接拉起)    │            │  (GUI 直接拉起)    │           │  PacketTunnelProvider (Swift)│
└─────┬────────────┘            └─────┬────────────┘           │  内嵌 libbox (sing-box 核心库) │
      │ TUN: NE 系统扩展+libbox        │ TUN: wintun 驱动        └────────────────────────────┘
      │ （与 iOS 共用隧道代码）          │ + 加固服务模式（见 §3.4）
      ▲                               ▲
      └───────────────────────────────┘
```

### 3.2 分端选型

**桌面端（macOS + Windows）：Tauri 2 + React/TypeScript（同一壳）**
- 包体 ~10MB、内存低（直击 Electron 系最大抱怨）；Rust 侧天然与共享核心同一语言。
- 前端：React 18 + Zustand + Tailwind + shadcn/ui + Recharts，Vite 构建，CSS variables 明暗主题。
- macOS 用系统 WKWebView，Windows 用系统 WebView2（Win10 1809+ 自带），两平台均无 Linux WebKitGTK 一类问题。

**iOS 端：SwiftUI + NetworkExtension（原生，不用 Tauri iOS）**
- VPN 类 App 的生命周期（NEVPNManager 配置、VPN Profile 授权流程、App Store 审核）深度绑定原生，SwiftUI 是唯一稳妥路线；移动端 UI 与桌面差异大，复用收益低。
- **内核**：iOS 进程内只能跑库不能跑子进程 → 用 **libbox**（sing-box 官方移动库，SFI 同款）。mihomo 的 gomobile 方案无官方维护，不采用。
- NetworkExtension PacketTunnelProvider 承载流量；App 与扩展通过 App Group 共享配置。

**共享大脑：crossbow-core（Rust crate）**
- 承载：Profile/订阅管理、覆写链（merge/JS）、配置渲染（→ mihomo YAML 或 sing-box JSON）、备份快照、iCloud 同步协议、诊断逻辑。
- macOS / Windows 经 Tauri 直接调用；iOS 经 **UniFFI** 生成 Swift 绑定、以静态链接进 App 与扩展（扩展内只做只读配置消费）。

### 3.3 内核策略

| 场景 | 内核 | 理由 |
|---|---|---|
| macOS / Windows 普通模式（系统代理） | mihomo sidecar（默认） | Clash 生态事实标准、订阅兼容性最好 |
| macOS / Windows 备选引擎 | sing-box sidecar | 新协议快、与 iOS 同构 |
| macOS TUN（M1） | NE 系统扩展 + libbox | 与 iOS 共用隧道代码；原生权限模型 |
| Windows TUN（M1.5） | mihomo/sing-box + wintun 驱动 | 两内核都原生支持 wintun；驱动由内核托管 |
| iOS | libbox（唯一） | 平台限制 |

- `CoreAdapter` 抽象：启动参数、配置转换、API 探测、日志归一化，GUI 不与内核强耦合。
- mihomo 为 GPL-3.0：项目整体以 GPL-3.0 分发，sidecar 外部进程调用（不链接），严格遵守上游名称要求。sing-box（libbox）许可条款在 iOS 分发前需复核最新版附加条款。

### 3.4 各平台系统能力

| 能力 | macOS | Windows |
|---|---|---|
| 系统代理 | `networksetup`（枚举所有网络服务逐一设置/还原，退出崩溃兜底还原） | 注册表（`Internet Settings`）+ `InternetSetOption` 刷新 WinINet/WinHTTP |
| TUN | NetworkExtension 系统扩展 + libbox（需向 Apple 申请 entitlement；Developer ID 公证分发）。**不自研提权服务** | wintun 驱动（内核托管加载）+ **加固服务模式**：Windows 服务（SYSTEM）只暴露 `start-core` / `stop-core` / `status` 三个动词，named pipe 通信 + 调用方签名/ACL 校验 + 内核二进制哈希校验（吸取 Verge `clash-verge-service` LPE 教训），UAC 仅装服务时弹一次 |
| 内核控制通道 | Unix domain socket + 随机 secret | named pipe + 随机 secret（不用固定端口 HTTP） |
| 开机自启 | SMAppService (LoginItem) | 注册表 Run / 计划任务 |
| 托盘 | Tauri tray 插件 | 同左 |
| 深链接 | `crossbow://import?url=...` 注册 | 注册表 URL Protocol |
| 跨设备同步 | iCloud（iOS ↔ macOS） | Windows 暂不同步，配置文件可导入导出（M3 评估 OneDrive/WebDAV 桥） |
| 密钥存储 | Keychain | Windows 凭据管理器（Credential Manager，经 `keyring` crate） |

**Windows 服务模式安全细则**（对照 Verge LPE 教训）：
- 服务只接受本机 named pipe 连接，pipe DACL 限定管理员与已安装用户组；连接时校验调用方进程签名（Authenticode）与路径。
- 服务**不接受任意命令行**：仅三个动词；内核路径白名单限定在 Program Files 应用目录，启动前校验哈希（清单随应用签名分发）。
- 卸载清理：反注册服务、删除防火墙规则、还原系统代理；服务看门狗在 GUI 消失后按策略 stop-core 并还原代理。

**iOS（M2）**：App Group 配置共享、NEVPNManager 生命周期、Keychain。

### 3.5 数据模型（核心实体，双端共享 schema）

```
Profile        订阅/本地配置档案: id, name, type(remote/local), url, interval,
               content(YAML), chain(覆写链引用), lastUpdated, trafficInfo
Override       覆写: id, name, type(yaml-merge|js), scope(profiles[]), enabled, content
EngineConfig   内核: engine(mihomo|singbox), channel(stable|alpha), mixedPort, tunEnabled...
SyncState      iCloud 同步账本: 实体版本向量、冲突策略(最新胜出+快照兜底)
Diagnostics    诊断报告: 步骤、结果、修复动作
```

**升级不丢配置**：数据目录版本化 schema + migration 测试强制；每次升级前自动快照（保留 N 份）；Profile 与覆写分离存储。

### 3.6 分发与更新

- macOS：Developer ID 签名 + 公证，Tauri updater 自动更新；Homebrew cask。
- Windows：NSIS/MSI 安装包；代码签名证书（EV 或 OV + 渐进声誉积累，无签名时明示 SmartScreen 提示与哈希校验）；Winget / Scoop 清单；Tauri updater 自动更新。
- iOS：TestFlight（开发期）→ App Store（$0.99 或免费，评估中）。
- 内核二进制：下载校验上游哈希，版本锁定 `core-versions.json` 走 PR 审查；libbox 通过 SPM 二进制 target 引入。

---

## 4. 产品设计

### 4.1 macOS 信息架构（侧边导航）

```
首页        仪表盘：总开关、当前出口、实时上下行曲线、今日流量、连接数、TUN/系统代理状态灯
代理        策略组列表（节点卡片 + 单击测速 + 右键固定），全局搜索
配置(Profile) 订阅卡片（流量条、更新时间、右键更新），导入(粘贴/文件/深链接)，覆写链管理
连接        实时连接表：规则/进程/目标/上下行，过滤、暂停、单条断开
日志        内核日志分级过滤 + 虚拟滚动
规则        规则集只读视图 + 命中统计（v1.0）
设置        内核 / 系统代理 / TUN / 外观 / 热键 / iCloud 同步 / WebDAV / 关于
```

### 4.2 iOS 信息架构（TabView）

```
首页        连接总开关（大按钮）、当前节点、实时速率、已用流量
代理        策略组 → 节点选择（原生列表 + 测速）
配置        订阅管理（导入/更新）、与 macOS 同步状态
更多        设置、诊断向导、日志、关于
```

### 4.3 功能分期

**M0 · macOS MVP（当前周期，6–8 周，可日用）**
- Profile：导入/更新/自动更新/基础编辑；merge 覆写
- 代理：组/节点测速、选择、固定；首页仪表盘精简版
- 系统代理（networksetup 全服务设置/还原）+ 托盘（模式切换、总开关、退出）
- 连接页（只读）+ 日志页 + 设置基础项
- 深链接导入、开机自启、明暗主题、中英双语
- **全程 CI 守护 Windows 可编译**（不写破坏 Windows 的平台代码）

**M1 · macOS 完整版（口碑线）**
- TUN：NetworkExtension 系统扩展 + libbox（含 entitlement 申请流程）
- JS 脚本覆写、覆写示例市场；WebDAV + 本地快照备份
- 轻量模式（GUI 退出留核）；连接页完整交互（断开、进程聚合）
- 网络诊断向导；sing-box 引擎切换；图表完整版

**M1.5 · Windows 版**
- Windows 打包链路：NSIS 安装包 + 签名 + updater
- 系统代理（注册表 + WinINet 刷新）、深链接、自启、凭据管理器
- TUN：wintun + 加固服务模式（§3.4 安全细则全套落地）
- 与 macOS 共享全部核心测试；双平台 QA 走查（高分屏/DPI、杀软误报抽测）

**M2 · iOS 版（差异化线）**
- SwiftUI App + PacketTunnelProvider（libbox）+ App Group 配置共享
- crossbow-core 经 UniFFI 静态链接；iCloud 双端同步（Profile/选择/设置）
- TestFlight 公测 → App Store 上架

**M3 · 打磨**
- 规则命中统计；请求复制 cURL；DNS 泄漏检测面板；iOS 小组件/快捷指令；Windows OneDrive/WebDAV 同步桥评估

### 4.4 三张「王牌交互」

1. **网络诊断向导**：一键「我不能上网」→ 按序检查（系统代理生效性 / TUN 路由 / DNS 泄漏 / 节点连通性 / 内核日志错误），每步给出可点击修复动作。
2. **状态一致性守护**：退出/崩溃/升级任何路径都保证系统代理还原；托盘反映真实网络走向。
3. **设置项解释化**：每个开关带一行「这会做什么」；「系统代理 vs TUN」二选一引导。

---

## 5. 工程架构

### 5.1 仓库结构（monorepo）

```
crossbow/
├─ Cargo.toml               # Rust workspace
├─ rust-toolchain.toml      # 固定 stable 工具链
├─ package.json             # pnpm workspace
├─ crates/
│  └─ crossbow-core/        # 共享大脑：profile/覆写/渲染/备份/诊断（三端复用）
│     └─ uniffi/            # (M2) Swift 绑定生成
├─ apps/
│  ├─ desktop/              # macOS + Windows Tauri 2 应用（同一壳）
│  │  ├─ src/               # React UI（pages/ stores/ ipc/）
│  │  └─ src-tauri/         # Rust 壳：内核生命周期、sysproxy、托盘、深链接
│  └─ ios/                  # (M2) SwiftUI App + PacketTunnel 扩展
│     ├─ project.yml        # XcodeGen 工程定义
│     └─ README.md          # libbox 集成与签名说明
├─ service/                 # (M1.5) Windows 加固服务本体（Rust）
├─ docs/                    # DESIGN / MVP / WIREFRAMES
└─ .github/workflows/       # macOS + Windows CI
```

### 5.2 进程模型（桌面端）

- 普通模式：GUI(Rust) 直接以子进程拉起内核，uds（macOS）/ named pipe（Windows）+ secret 控制，WS 订阅 traffic/logs/connections，**Rust 侧节流聚合后转发前端**（连接页 ≤2fps，直击卡顿痛点）。
- TUN：macOS 经 NE 扩展（M1）；Windows 经服务模式拉内核（M1.5）。
- 轻量模式（M1）：GUI 退出，内核由 launchd user agent（macOS）/ 计划任务（Windows）托管存活，托盘常驻。
- 单实例锁 + 第二实例深链接转发。

### 5.3 质量与 CI

- crossbow-core：profile 迁移、覆写链、渲染单测强制；`cargo clippy --deny warnings`。
- 前端：Vitest + Playwright（webview E2E）。
- CI（macOS + Windows 矩阵）：lint + test + debug 构建产物；Windows job 从 M0 起保持可编译，防止平台回归。
- 发布：macOS Developer ID 签名 + 公证；Windows NSIS + Authenticode 签名 + updater 产物。
- iOS CI（M2）：xcodegen + xcodebuild test（需自托管或 macOS runner + 证书）。

### 5.4 风险清单（v3）

| 风险 | 等级 | 对策 |
|---|---|---|
| NE entitlement 审批不确定 | 高 | M0/M1.5 不依赖它；并行提审；降级方案为最小特权 helper |
| Windows 服务模式被披露 LPE | 高 | 最小动词面 + pipe DACL + 调用方签名校验 + 内核哈希校验 + 上线前外部审计 |
| wintun 驱动安装/兼容（旧 Win10、杀软拦截） | 中 | 内核托管驱动加载；安装器预装驱动；杀软误报白名单指引文档 |
| Windows SmartScreen / 无签名分发劝退 | 中 | 尽早购买代码签名证书；发布页提供 SHA256 校验 |
| WebView2 运行时缺失（LTSC/精简系统） | 中 | 安装器内嵌 WebView2 bootstrapper |
| libbox 与 mihomo 配置语义差异 | 中 | 渲染层双 target（YAML/JSON）+ 差异测试集 |
| mihomo GPL 与名称要求 | 中 | GPL-3.0 分发、sidecar 不链接、遵守改名要求 |
| App Store 审核（VPN 类） | 中 | 参照 Shadowrocket/Stash 先例；备选 TestFlight/Developer ID 分发 |
| UniFFI 边界性能/类型摩擦 | 低 | 核心接口保持粗粒度 |
| 上游内核再删库 | 低 | 双内核抽象；内核镜像缓存到发布渠道 |

---

## 6. 参考资料

- Clash Verge Rev：<https://github.com/clash-verge-rev/clash-verge-rev> · 文档 <https://www.clashverge.dev>
- Sparkle（Mihomo Party 分支）：<https://github.com/xishang0128/sparkle>
- FlClash：<https://github.com/chen08209/FlClash> · Hiddify：<https://github.com/hiddify/hiddify-app>
- mihomo：<https://github.com/MetaCubeX/mihomo> · sing-box / libbox：<https://github.com/SagerNet/sing-box>
- mihomo vs sing-box 选型：<https://supernet.day>（"Clash vs sing-box：2026 年该怎么选？"）
- NetworkExtension / PacketTunnelProvider：Apple Developer 文档（NEPacketTunnelProvider, NEVPNManager）
- wintun 驱动（Windows TUN）：<https://www.wintun.net/> · Windows 服务最佳实践（named pipe DACL / 服务安全）：<https://learn.microsoft.com/windows/win32/services/service-security>
- Surge 手册（Dashboard/策略组设计基准）：<https://manual.nssurge.com>
- Verge 服务模式 LPE 分析（反面教材）：blog.evi1s.com
- V2EX / Linux.do 用户反馈：内存 10GB+（BT 场景）、切节点卡死、"bug 太多转 FlClash" 等

---

## 7. 路线执行

| 步骤 | 状态 |
|---|---|
| 1. MVP 范围冻结（docs/MVP.md）+ 三页 wireframe（docs/WIREFRAMES.md） | ✅ 已完成 |
| 2. monorepo 骨架：Rust workspace + crossbow-core + Tauri 2 桌面壳 + CI | ✅ 已完成 |
| 4. 内核 sidecar 生命周期（启动/停止/崩溃自动重启/就绪探测） | ✅ S2 完成，真实 mihomo 冒烟通过 |
| 4b. Profile 导入/更新/激活/删除 + 订阅流量头解析 + 运行时配置注入 | ✅ S2 完成 |
| 3. Spike：macOS 系统代理设置/还原 + 崩溃兜底（networksetup） | ⏭ S3 |
| 4c. 托盘（模式切换/总开关）+ traffic/logs/connections WS 节流转发 | ⏭ S3/S4 |
| 5. NE entitlement 向 Apple 提交申请（M1 前置，审批周期长，尽早启动） | ⏭ 需开发者账号 |
| 6. Windows 前置：Windows CI job、icon.ico | ✅ 已完成；NSIS 打包试跑 ⏭ M1.5 |
| 7. M1.5 Spike：Windows 服务模式 named pipe + 签名校验（在 Windows 机器上验证） | ⏭ M1 后 |
| 8. M2 前置：UniFFI 绑定 spike、libbox SPM 集成 spike（需完整 Xcode） | ⏭ M1 后 |
