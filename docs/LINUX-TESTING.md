# Linux 第一阶段：CachyOS + niri/Wayland

关联 #213。Linux 目前提供显式 HTTP/SOCKS 代理；不修改系统代理、TUN、路由或 DNS。
Linux 能力由 Rust 返回，首页、设置与托盘据此禁用未支持入口；直接调用系统代理/TUN 命令也会失败。
订阅或覆写中的 TUN 入站在启动/热重载前被拒绝。应用自动更新在提供 Linux 发布产物前禁用。

## 本机开发与构建

依赖 Node.js 20+、pnpm、Rust stable、GTK 3、WebKitGTK 4.1、AppIndicator 和常规 C 构建工具。
本次使用 CachyOS x86_64、niri/Wayland，GTK 3.24.52、WebKitGTK 2.54.2。

```sh
pnpm install --frozen-lockfile
python3 scripts/ci/download_sidecar.py x86_64-unknown-linux-gnu
cd apps/desktop
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
  pnpm tauri build --debug --no-bundle \
  --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

产物位于仓库根目录 `target/debug/crossbow-desktop`，旁边的 `mihomo` 是已核对上游 SHA-256 的 v1.19.32 内核。
Linux 内核下载映射同时支持 sing-box；第一阶段主验收内核为 mihomo，不能据此声明 sing-box 或其他架构完成日用验证。

本机运行可用 `GDK_BACKEND=wayland target/debug/crossbow-desktop`。
已有代理占用 7890 时，先在设置中选择未占用的混合端口，再导入配置并开启首页的“显式代理”。
应用需要主动将 HTTP 或 SOCKS 代理设到显示的回环地址；启动内核不等于浏览器已自动经过 Crossbow。

## 自动化原生验证

测试使用真实 WebKitGTK 窗口、Tauri IPC 和 mihomo，不替换前后端。
需要 `tauri-driver`、`WebKitWebDriver`、`curl` 与图形会话；可将驱动安装在单独目录：

```sh
cargo install tauri-driver --locked --root /tmp/crossbow-linux-tools
GDK_BACKEND=wayland python3 scripts/linux/smoke_desktop.py \
  --application target/debug/crossbow-desktop \
  --driver /tmp/crossbow-linux-tools/bin/tauri-driver \
  --output /tmp/crossbow-linux-loopback
```

脚本创建临时 XDG 配置/数据/缓存目录、空闲端口和本地订阅/HTTP 上游。
它从真实界面导入订阅、启停内核、选择节点，并通过 curl 的 HTTP 与 SOCKS5h 两条入口访问本地上游。
同时验证失败更新保留旧配置、真实日志展示、诊断跳过不支持项和停止后释放端口。
运行前后比较 IPv4/IPv6 路由、规则及 `/etc/resolv.conf` 的摘要，最后关闭验证实例并删除临时数据。
输出包含 `report.json`、原生 WebView 截图与驱动日志；均使用脱敏测试数据。

若已存在可用的本机 HTTP 代理，可额外验证外网链路：

```sh
GDK_BACKEND=wayland python3 scripts/linux/smoke_desktop.py \
  --application target/debug/crossbow-desktop \
  --driver /tmp/crossbow-linux-tools/bin/tauri-driver \
  --upstream-port 7890 \
  --output /tmp/crossbow-linux-external
```

此时链路为 **测试请求 → Crossbow 混合端口 → Crossbow mihomo → 现有本机代理 → 外网**。
测试不读取现有代理的订阅、凭据或控制器，也不修改它的设置；验证 HTTP 204 与 HTTPS CONNECT。
它证明 Crossbow 的真实代理链路工作，不能替代直接连接远端节点或独立 TUN 的验收。

## 回归与 CI

```sh
cargo test --locked -p crossbow-core
cargo test --locked --lib --manifest-path apps/desktop/src-tauri/Cargo.toml
node --experimental-strip-types --test apps/desktop/src/components/nodeFlags.test.mjs apps/desktop/src/components/fitGrid.test.mjs
python3 -m unittest discover -s scripts/ci/tests -v
```

`build.yml` 增加 Ubuntu 24.04 Linux 编译/测试任务，保留 macOS/Windows 原有验证。
CI 编译通过不是 niri/Wayland 实机通过；本机原生验证结果单独记录。

## 2026-10-30 本机验证结果

基于 `8f292be` 加本 PR 变更，在 CachyOS/niri 的原生 Wayland 会话中执行，未使用 Xvfb 或浏览器模拟页面。

| 项目 | 结果 |
|---|---|
| TypeScript/Vite、Tauri debug 原生程序 | 构建通过 |
| 桌面 Rust 单测 | 97 通过 |
| 共享核心单测 | 50 通过；另 2 项真实 store/sing-box 测试因未提供 `CROSSBOW_STORE` 提前返回，不计入真实验证 |
| 节点旗帜/网格组件测试 | 6 通过 |
| CI/发布脚本测试、源版本检查 | 20 通过，版本检查通过 |
| 本地 HTTP 上游闭环 | UI 导入/启停/选节点、HTTP/SOCKS5h、日志、更新失败恢复与 TUN 配置拒绝通过 |
| 现有 FlClash 7890 作为上游 | HTTP 与 SOCKS5h 请求均返回 204；HTTPS CONNECT 到 GitHub 返回 200 |
| 真实网络诊断 | 6/6 适用项通过，系统代理单独标为不适用 |
| 网络保全与退出 | 两次原生验证路由/DNS 摘要均未变化；停止后混合端口释放 |

最终外网验证程序 SHA-256：`3436e5b2fda56fbc6904e2a28f304308be12e03ef368e5f426e4a34866b8104c`。
本机报告与截图保存在工作树 `target/linux-validation/`，构建产物不提交到 Git。
首轮截图发现新增显式代理行超出接管卡片高度，已仅针对 Linux 调整布局，并在最终原生验证中检查控件边界。

## 尚未覆盖

- Linux 系统代理、TUN、特权服务、VPN 共存检测与网络恢复。
- 后台浮层不抢焦点、托盘速率、休眠恢复、开机自启及深链接的完整日用验收。
- sing-box 真实联网、多种远端协议、其他发行版/桌面/CPU 架构。
- AppImage/DEB/RPM、安装卸载、公开发布与应用自动更新。

现有托盘可用于重新打开窗口和退出；窗口关闭驻留行为保留。对无托盘宿主的桌面尚未作支持承诺。
诊断的系统代理项显示“不适用”，不计入通过数；其余网络检查按真实结果显示，不以编译或端口监听代替联网成功。

## 第二阶段先行修复（2026-10-30，#216）

这些修复放在 `codex/linux-network`，第一阶段 PR #214 的提交保持 `f6dccc1`，保留已启动的 CI。
网络接管和特权服务仍未开启。

- Linux 配置检查仅接受未设置、空值或明确为 `false` 的 `tun.enable`，拒绝 `yes/on`、字符串及其他含糊类型。检查前复用 YAML 库处理合并键，不能确认安全的剩余合并也会拒绝。
- 首页和诊断页停止失败时由已有后端回调报告错误；诊断不继续重启，首页重新读取真实状态。启动预检错误仍会显示。
- Linux 显式代理复用原有代理控制行，删除多余控制行和专用高度样式；未增加依赖或通知、进程管理框架。

验证结果：桌面 Rust 单测 99 项通过，新增的布尔兼容与 YAML 合并用例在修复前失败、修复后通过；前端及 Tauri 原生程序构建通过，完整分支差异的 `git diff --check` 通过。
原生脚本再次验证了订阅更新中的 `true`、`yes` 和合并键 TUN 配置拒绝、旧配置保留、HTTP/SOCKS5h 204、HTTPS 200、诊断和正常停止，路由/DNS 摘要未变。

界面检查使用隔离浏览器和临时 IPC 数据：停止失败只收到后端一条错误，诊断不继续启动；启动预检失败保留错误及关闭状态，成功启停不额外通知。五种尺寸（1120×720、880×560、1440×960、1120×1400、1920×600）与中英文、深浅主题的 20 个组合均无页面横向溢出，接管控件未超出面板。
这组故障注入验证的是前端分支，不代表真实操作系统拒绝终止进程的场景已实测。

本机证据位于 `target/linux-validation/fixes-native/` 和 `target/linux-validation/fixes-ui/`；原生程序 SHA-256 为 `c26549920c48aab4bbf7b9cb83ffa93ae4d47ff36923a09851fdee3576f7e3ee`。


## 第二阶段首轮：受控 TUN 与恢复（2026-10-30，#216）

实现与安装步骤见 [LINUX-NETWORK.md](LINUX-NETWORK.md)。沿用现有 `crossbow-service` 和 `CoreManager`，没有新增通知框架、进程框架或第三方包；Linux 仅复用已有的 libc、serde 与 socket2。第一阶段 PR #214 已由维护者合并，提交仍是 `f6dccc1`。

| 验证层 | 结果与边界 |
| --- | --- |
| 桌面 Rust | 105 项通过，其中服务客户端 6 项覆盖权限、身份、错误帧、超时和清理确认 |
| Linux 服务 | 13 项通过，覆盖受控配置、provider 路径、端口占用、资源匹配与未确认创建记录 |
| 构建与静态检查 | 前端、Tauri 原生程序、Linux 服务构建通过；差异空白、安装脚本及 polkit XML 检查通过 |
| 隔离网络 | 11 项真实服务/内核检查通过，IPv4/IPv6 TCP、UDP DNS、正常停止、重启复用、端口冲突、桌面 EOF、内核 SIGKILL、外来规则保留、所属规则丢失、独立恢复入口均覆盖 |
| niri/Wayland 实机 | 14 项通过，含显式代理链路、现有 FlClash/VPN 冲突在授权前被拒绝、原内核保留、设置页服务状态及正常退出 |
| 浏览器故障注入 | 7 项通过；TUN 失败不重复通知，关闭失败保留开启态并可重试；中英文、深浅主题和五种尺寸共 20 组布局通过 |
| 系统级授权与实网 TUN | **未验收**。没有在这台机器安装服务、触发管理员授权或开启宿主 TUN。隔离测试没有运行真正的系统级 systemd/polkit |

隔离与原生两组报告的宿主路由/DNS 摘要均未变化。隔离核心实际只有 `CAP_NET_ADMIN`；提供的 resolvectl 探针未被调用。`ExecStopPost` 测试运行了同一个恢复入口，但没有证明 systemd 会在所有故障场景自动调用它。前端故障注入也不代表真实系统拒绝终止进程的场景已实测。

完整服务 lint 命令为 `cargo clippy --locked -p crossbow-service --all-targets -- -D warnings -A clippy::assertions_on_constants`，通过；仅排除了原有 `policy.rs` 常量断言提示，没有为此改动 Windows 策略代码。

本机证据：`target/linux-validation/network/report.json`、`network-native/report.json`、`network-ui/report.json`（均在 `target/linux-validation/` 下）。实测产物 SHA-256：

- 桌面：`065cf781c36e665a2ed1ea786d4230048da6f5ba2065ed079411032b261f249f`
- 服务：`17b789334077dbea84d1dd42921ee4caba969634653a47990c8efba12d5d26bb`

这些结果支持代码评审及下一步专用环境验收；第二阶段完整日用验收仍未通过。

## 第二阶段补充：系统授权与维护者反馈（2026-10-30，PR #223）

提交 `a1ddc826` 随后在一次性 QEMU/KVM 虚拟机完成 15 项补充检查：真实 systemd/polkit、按用户运行且只有 `CAP_NET_ADMIN`、服务身份确认、自动 `ExecStopPost`、跨用户连接拒绝、运行中安装拒绝、外部公网地址代理防回环，以及 KDE 图形授权的 Cancel/OK。取消和成功授权前后，原真实 mihomo 显式代理仍可用。虚拟机图形环境使用 Xorg，未据此声称宿主 Wayland 授权窗口已验证；报告在 `target/linux-validation/system-acceptance/report.json`。该提交的 CI run `41021486895` 六个 job 均通过。

维护者随后指出严格反向路径检查、GeoSite 文件大小写和节点选择缓存三项问题。本轮修复保持原服务与网络恢复流程，验证如下：

- **反向路径检查**：在 KVM guest 将外部代理放入另一个网络命名空间，使用公共地址 `203.0.113.2`。旧版在 `all=0 / external0=1` 时返回 active，但请求失败，内核 `IPReversePathFilter` 丢包计数增加 6；同环境改为宽松模式后请求成功且不再增加。新共享预检拒绝有效严格模式，原显式代理 PID、代理请求、路由/DNS 和 sysctl 均保持不变，未创建授权服务。另核验 `(all,接口)` 为 `(1,0)` 时拒绝、`(1,2)/(2,1)/(0,2)` 时放行。
- **GeoSite**：使用只含 CN/TEST 的合法微型数据文件，仅保留小写 `geosite.dat`，并将下载地址指向不可连接的本地端口。旧服务实测启动失败；通过真实安装脚本换成新服务后，同配置能离线加载 `GEOSITE` 规则。复制测试同时覆盖普通应用实际使用的 `geoip.metadb` 和已有大写兼容名。
- **节点选择**：真实 mihomo 的普通代理选择进入 TUN 后保留；TUN 重启、内核 SIGKILL、再切回普通模式后选择与原数据库 inode 均保留。现有合法 `0644` 缓存不会被截断或收紧权限，新文件用 `0600`；符号链接目标和占用中的数据库会被拒绝。
- **缓存就绪**：慢 provider 证明 `/version` 的 200 响应可能早于缓存初始化，不能单凭控制器可用就报告接管完成。真实 systemd 服务在控制器已就绪、缓存尚未打开时仍等待，尚未安装优先级 `12020` 的规则。放行 provider 后，合法缓存正常启动；损坏缓存使共享链接被替换时明确失败并清理，原坏数据库的内容摘要不变。

本轮桌面后端 105 项、服务 19 项单测通过；服务构建及 clippy 通过。扩展后的 `scripts/linux/smoke_network.py` 共 16 项通过，宿主路由/DNS 摘要未变。测试产物与报告位于 `target/linux-validation/review-network/`、`review-rp-filter/`、`review-assets/` 和 `cache-review/`；独立测试基础设施没有提交到仓库。服务实测 SHA-256 为 `982b8e1ceb6b49f7f55837ee8d607fc5d0849b02874926478c87a1d1236e11fb`。

当前宿主没有安装服务或启用 TUN，FlClash 和 VPN 未改动。宿主实网日用与公开发布验收仍不标记为通过。

## 2026-11-01 日用验收（#255，原生 Wayland）

基于 `1d7cf54`，在 CachyOS x86_64 + niri 26.04 的真实 Wayland 会话执行。先直接运行官方 v0.12.2 Linux 压缩包，再验证本次三处修复的本地 debug 构建；没有以浏览器模拟或隔离虚拟机代替这轮桌面验证。

发布包 `Crossbow_0.12.2_linux_x86_64.tar.gz` 的 SHA-256 与 GitHub Release 元数据一致：`b15cbc43cc1d5e8e4e87295c65f12afeb262804862d2283b48a7e01b5ddf0fe2`，桌面 ELF 的动态依赖均能解析。包内桌面程序 SHA-256 为 `71d0d0c2620babeeee807cabf66d07c613eff8da4a4f8e1ba33a79ad7eea0433`。

测试实例使用独立 HOME、XDG 配置/数据/缓存目录、空闲端口与本地脱敏订阅。Linux 单实例 D-Bus 名不随 XDG 目录变化，因此启动前先确认没有其他 Crossbow 占用，发现占用则退出测试。自启动插件直接写 HOME 下的 `.config/autostart/Crossbow.desktop`，只隔离 XDG 不足以保护真实用户配置。

| 验收项 | 结果与范围 |
| --- | --- |
| 官方发布包基础回归 | 14 项通过：实际界面与 IPC、显式 HTTP/SOCKS、HTTPS、日志、诊断、无效更新恢复与冲突拦截 |
| 正常关窗驻留 | 实际 niri 关窗与原生托盘唤回连续 3 次通过，测试代理持续可用 |
| 托盘菜单 | 真实 StatusNotifierItem/DBusMenu 注册、直连/规则模式切换、配置切换、打开窗口、退出均验证 |
| 轻量关窗 | 发布包会停掉后台日志，重开后仍未恢复；修复后保留后台日志和流量历史，仅停止已销毁页面的订阅 |
| 关闭唯一通知窗口 | 发布包会退出应用并停止内核；修复后收起当前卡片，应用、代理、未读未解决历史均保留 |
| 深链接 | 精确二进制 argv 的第二次启动导入原本可用；冷启动导入原本丢失，补读启动 URL 后通过。没有注册或调用宿主 URL 关联 |
| 普通退出与重启 | 偏好、活动配置、节点选择保留；再次启动的显式代理可用；托盘退出后 PID 文件与监听端口释放 |
| 自启动 | 仅在隔离 HOME 中验证真实插件写入及删除条目，未启用宿主启动项，未验证真实登录执行 |
| 后台浮层 | 在 niri 中确实浮动；首次和再次出现均抢占焦点，**未通过**，跟踪 [#256](https://github.com/Richard-Zhang1019/Crossbow/issues/256) |

`smoke_desktop.py` 已更新当前 Button 选择器和实例/HOME 隔离；新增 `smoke_daily.py` 覆盖本轮原生窗口、托盘、缓存与协议路径。修复构建的日用检查共 24 项，22 项通过、2 项焦点检查失败，报告保留 `passed=false`，不能写成完整日用验收通过。焦点测量先在可见主窗口发出测试通知，再关闭它并聚焦自有参考窗口，确认浮层尚未映射后才观察；期间不操作 WebDriver。当前 niri 开启了 `focus-follows-mouse`，没有改动这些用户规则。

复现命令（需要 `tauri-driver`、`WebKitWebDriver`、`niri`、`busctl`、`gdbus`、`zenity` 和真实图形会话）：

```sh
GDK_BACKEND=wayland python3 scripts/linux/smoke_desktop.py \
  --application /absolute/path/to/crossbow-desktop \
  --driver /absolute/path/to/tauri-driver \
  --upstream-port 7890 --output /tmp/crossbow-release-smoke
GDK_BACKEND=wayland python3 scripts/linux/smoke_daily.py \
  --application /absolute/path/to/crossbow-desktop \
  --driver /absolute/path/to/tauri-driver \
  --output /tmp/crossbow-daily
```

后一命令当前会因 #256 返回非零；逐项结果与清理后的网络摘要仍会落盘。复跑可复用该输出目录旁 `fixture-assets/` 中由测试实例下载的 Geo 数据，避免每次重新下载；不读取现有代理的数据。

本地构建的前端、Tauri 原生程序和 106 项桌面后端单测通过。修复构建也重新通过 14 项基础原生回归。修复构建 SHA-256：`d27b51d569135c4794ccb7d27124f8309fce4a52d797bfb97425db97e483a61e`。证据位于本工作树 `target/linux-daily/evidence/` 的 `release-smoke/`、`release-daily/`、`fixed-smoke/`、`fixed-daily/`，包括 JSON 结果、原生截图和驱动日志；这些测试产物不提交。

最终核对确认路由/DNS 摘要、记录的原代理/VPN 进程身份与全部 `rp_filter` 值一致，FlClash 原核心仍运行；测试应用与内核均已退出，宿主未安装网络服务。汇总 `report.json` 保留 `completed=true / passed=false`，前者表示这轮检查与清理完成，后者表示焦点问题仍未解决。

本轮未运行宿主 TUN、系统代理切换、休眠/唤醒或真实登录自启动，未覆盖其他发行版与桌面。Linux 托盘的速率、节点选择和 TUN 菜单目前也没有实现。上述限制继续保留，宿主实网日用验收仍未完成。
