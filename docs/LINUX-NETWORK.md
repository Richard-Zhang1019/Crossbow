# Linux 网络接管（第二阶段，#216）

面向 CachyOS x86_64 + niri/Wayland。显式代理仍可独立使用；TUN 需要单独安装网络服务，当前只支持随安装提供的 mihomo。v0.11.5 起提供实验性 x86_64 压缩包；系统代理设置、AppImage/DEB 安装包及自动更新尚未开放。

## 开启前会发生什么

桌面先检查现有隧道、代理进程、IPv4/IPv6 策略路由和保留路由表。默认发现 FlClash、WireGuard、Tailscale 等冲突时直接返回原因，保留正在运行的显式代理；不会关闭其他程序、修改它们的配置或抢占端口。服务在真正启动前再检查一次，运行中发现新的冲突或自身路由丢失时停止本次接管。

可用 IPv4 接口还需通过反向路径检查预检。Linux 使用 `max(conf/all/rp_filter, conf/接口/rp_filter)` 作为有效值；值为 `1` 时，这种 TUN 策略路由会使公网代理回包被严格检查丢弃。因此当前实现明确拒绝接管，报出接口与参数，原显式代理继续运行。值为 `2` 的宽松模式不属于这个限制；不能读取或确认参数时也拒绝。Crossbow 不修改这些 sysctl，需由管理员结合现有网络策略处理。[内核参数说明](https://docs.kernel.org/networking/ip-sysctl.html#rp-filter-integer)

检查通过后才请求系统授权。桌面保持普通用户权限，`pkexec` 只调用固定安装路径的 `crossbow-service --authorize`。它从 `PKEXEC_UID` 取得调用用户，启动固定名称的 systemd 临时服务 `crossbow-tun.service`：

- 服务和 mihomo 以该用户身份运行，仅有 `CAP_NET_ADMIN`；不授予 root 文件访问权限，不给桌面或下载内核设置 setuid/file capability。
- Unix socket 位于 `/run/crossbow-tun/control.sock`，目录 `0700`、socket `0600`；两端验证所属用户，桌面还验证服务程序身份。请求仅支持启动、状态和停止，不接受可执行程序路径或任意命令。
- 服务读取用户配置并生成受控副本，限制文件大小和 provider 相对路径，固定回环控制器及随机密钥，覆盖网络接管字段。订阅和覆写不能自行启用额外 TUN。
- 桌面到服务的连接在整个 TUN 会话中保持打开。配置变化复用已有的服务重启流程，重新经过检查，不直接向特权内核热重载未经处理的配置。

服务安装与一次系统授权是两件事。取消授权或服务启动失败会报告错误，原来的显式代理继续运行。服务正常停止后保留最多 30 秒空闲时间，供配置重启复用；桌面重新启动不会根据旧开关自动接管网络。

已有的 `geosite.dat`、`geoip.metadb` 等数据文件会复制到受控目录，保留 Linux 文件名大小写。节点选择使用普通运行目录中的同一份 `cache.db`，会话目录只持有链接；退出或清理会话不会删除原数据库，普通代理与 TUN 切换也不会各留一份选择。服务验证缓存的所属用户、文件类型和占用状态，等待内核实际打开缓存后才报告就绪。若内核重建损坏数据库并断开共享链接，接管会失败并恢复，原缓存留给显式代理模式修复。

## WireGuard 分流共存（#379）

在 TUN 关闭时，从设置的「网络与内核」启用「保留 WireGuard 分流」，保持 WireGuard 已连接，再开启 Crossbow TUN。选项默认关闭，重启 Crossbow 后仍需手动开启 TUN。若网络服务早于此功能，先用下文的安装流程更新服务和 mihomo。

首版需要系统的 `wireguard-tools`、一个启用的原生 WireGuard 接口、主表中的直连 IPv4/IPv6 分流路由、可核实的 `AllowedIPs` 和固定对端地址。固定对端 IP 整体保留原路由。全隧道、分段默认路由、端点经另一个隧道、重叠 LAN 前缀、未知策略、多个 WireGuard 接口和 Tailscale 等其他 VPN 仍会拒绝接管。WireGuard 端点、接口或路由变化会触发退场，恢复原网络后可以手动重试。

共存模式只接管本机发出的未标记流量，保留外部转发及 VPN 原有路由。服务用专用表 `20221` 和优先级 `12019` 保留已核实的 WireGuard 前缀，固定端点在 Crossbow 表 `20220` 继续走原路由。受保护前缀的设备路由消失时，表 `20221` 的不可达后备路由在会话期间阻止回落到公网。撤回会话后恢复系统原有行为。

共存模式停用 mihomo 的通配 DNS 劫持，使用 `redir-host`。VPN 域名继续由系统的分流解析器处理，DNS 服务器应由已有 WireGuard 路由覆盖。系统解析器、VPN 配置、防火墙和 sysctl 仍由原有组件管理。DNS 的断线回退取决于系统策略。

## 安装

使用 Release 的 `Crossbow_<版本>_linux_x86_64.tar.gz` 时，完整解压到独立目录，以普通用户运行 `./run.sh`。包内 README 列出运行依赖；在解压目录执行以下命令安装可选服务：

```sh
sudo ./install-service.sh "$PWD/crossbow-service" "$PWD/mihomo"
```

升级前关闭 TUN，等待 `crossbow-tun.service` 空闲退出（最多约 30 秒），再从新版本目录运行安装脚本，同时更新服务和内核。不要以 root 启动图形应用。

从源码构建时按以下步骤：

需要 systemd、polkit、iproute2 和可用的 `/dev/net/tun`；niri 会话需要运行 polkit 认证代理。先从仓库根目录构建并检查产物，再明确执行安装：

```sh
cargo build --locked --release -p crossbow-service
sudo scripts/linux/install-service.sh \
  "$PWD/target/release/crossbow-service" \
  "$PWD/apps/desktop/src-tauri/binaries/mihomo-x86_64-unknown-linux-gnu"
```

脚本只安装 root 保护的 `/usr/lib/crossbow/{crossbow-service,mihomo}` 和 polkit action，不启动服务、不修改网络。运行中的 TUN 服务会阻止覆盖安装。缺少 sidecar 时先按 [Linux 测试说明](LINUX-TESTING.md) 下载仓库钉定版本。

安装后在设置页确认服务状态，再从首页或设置开启 TUN。Linux 的“服务已安装”不表示永久授权；系统仍会在新服务会话需要权限时确认。

卸载前先在桌面关闭 TUN，确认服务已停止且没有未恢复记录，再移除上述两个程序和 `/usr/share/polkit-1/actions/org.crossbow.tun.policy`。不要在接管过程中删除程序或恢复记录。

## 退出与恢复

服务只管理设备 `Crossbow`、路由表 `20220`、优先级 `12020` 和标记 `49451`，WireGuard 共存另用表 `20221` 和优先级 `12019`。自己添加的路由/规则均使用 protocol `242`。创建前检查占用，成功创建后记录精确资源与接口 ifindex，恢复时逐项核对、删除；不执行路由表全量 flush，不覆盖宿主快照。

普通关闭、桌面断连、内核退出、启动失败、冲突和路由漂移共用停止流程：先终止并回收自己的内核，再清理已确认资源。systemd 的 `KillMode=control-group` 和 `ExecStopPost` 提供进程异常退出后的兜底。清理失败会保留 `/run/crossbow-tun/network.json`，界面保留待确认状态，允许再次关闭重试。

若进程恰在内核接受创建命令、但成功记录尚未落盘的窗口中被杀，记录里的 `pending` 不能证明资源归属。此时服务保留记录并要求管理员核查，不自动删除这条未确认资源，也不允许下一次启动覆盖记录。接口 ifindex 已变化或出现重复匹配时同样停止自动恢复。`/run/crossbow-tun` 残留目录属于另一用户时不会自动改属主接管。

本实现不调用 resolvectl，不改 `/etc/resolv.conf` 或防火墙。公共目标经 TUN，私网、链路本地和组播等保留原路由；私网 DNS 也仍按原路由访问。WireGuard 分流共存以本页所列条件为限。DNS 处理遵循宿主解析器策略。

## 钉定内核的兼容处理

mihomo v1.19.32 使用 sing-tun v0.4.27。该版本即使 `auto-route: false`，非空 `inet6-address` 仍会生成额外的 IPv6 路由和 `oif` 规则。因此服务显式传入空数组，自行管理 IPv6 路由；gVisor 仍可转发 IPv6，已用真实 TCP 流量验证。内核的 `PATH` 限于 root 保护的安装目录，阻止旧版调用系统解析器工具。未改动其他平台的运行时注入。

依据：[sing-tun IPv6 规则](https://github.com/MetaCubeX/sing-tun/blob/v0.4.27/tun_linux.go#L496)、[gVisor 双栈处理](https://github.com/MetaCubeX/sing-tun/blob/v0.4.27/stack_gvisor.go#L91)、[pkexec 调用用户与授权](https://www.freedesktop.org/software/polkit/docs/master/pkexec.1.html)。

## CI 自动 VM 验收（#427）

GitHub Actions 的 ubuntu-24.04 runner 是一次性真实 VM：完整 systemd、polkit、可用的 `/dev/net/tun` 与真实宿主路由。`.github/workflows/linux-e2e.yml`（`workflow_dispatch` 手动触发；改动 Linux 验收脚本、服务、构建依赖或该工作流的 PR 自动触发）运行 `scripts/linux/e2e_vm.py`，复用 [smoke 脚本](../scripts/linux/smoke_network.py)的协议客户端与资源断言，覆盖以下矩阵：

| 验收项 | 方式 | 状态 |
| --- | --- | --- |
| 服务安装（root 保护路径 + polkit action） | `install-service.sh` + 属主/权限断言 | 已验（CI VM） |
| pkexec 系统授权与临时单元身份 | 真 `pkexec --authorize` + `systemctl show` 属性比对（MainPID/User/Transient/ExecStart） | 已验（CI VM） |
| TUN 资源注入（设备/表 20220/优先级 12020/protocol 242，双栈） | `ip -j` JSON 断言 | 已验（CI VM） |
| 核心权限收敛（仅 CAP_NET_ADMIN） | `/proc/<pid>/status` CapEff | 已验（CI VM） |
| 公网数据面 DIRECT 分流 | 真实 HTTP/HTTPS 请求 + 逐条匹配内核日志的连接地址、端口和 DIRECT 出口 | 已验（CI VM） |
| 私网保留原路由 | `ip route get` 对照 | 已验（CI VM） |
| 日志落盘 | 用户文件 0600 且非空 | 已验（CI VM） |
| 正常停止与配置重启复用 | socket 往返 + 资源恢复断言 | 已验（CI VM） |
| 服务 SIGKILL 后 ExecStopPost 自动恢复 | systemd 真实恢复路径 + 资源断言 | 已验（CI VM） |
| 卸载清理 | 移除二进制/策略后 pkexec 授权失败 | 已验（CI VM） |
| 宿主路由/DNS 前后一致 | 快照对照 | 已验（CI VM） |
| IPv6 公网数据面 | CI runner 无公网 IPv6 | 已知限制（双栈规则/路由已验，数据面由命名空间测试覆盖） |
| Wayland/niri 图形授权 | 桌面环境差异 | 不适用（此前一次性 KVM 已覆盖 KDE/Xorg 图形授权） |

公网 HTTPS 验收使用独立提供方的 `https://www.gstatic.com/generate_204` 与 `https://cp.cloudflare.com/generate_204`，避免把共享 runner 上 GitHub API 的拒绝响应当成 TUN 故障（#498）。首站传输失败或响应非 204 时复核另一站，两站均失败则验收失败；成功响应仍必须逐连接匹配 TUN 内核 DIRECT 日志，缺少日志不触发站点切换。TLS 校验、IPv4 请求和禁用显式代理保持开启。

脚本要求显式指定 `--disposable-vm`，并在检测到既有 Crossbow 服务、安装文件或 TUN 资源时停止。CI 通过预置 polkit 规则完成授权。在 `rp_filter` 为严格值时，脚本临时将其设为 2。退出时移除测试规则并恢复原值。

## 验证与未完成的实机验收

```sh
cargo test --locked -p crossbow-service
cargo test --locked --lib -p crossbow-desktop
cargo build --locked -p crossbow-service
python3 scripts/linux/smoke_network.py
```

网络脚本需要 bubblewrap、iproute2、util-linux、wireguard-tools、可用的 WireGuard 内核支持及允许创建的用户命名空间。它使用独立 user/PID/mount/network namespace、临时安装映射和真实 mihomo，不需要 sudo；宿主网络在前后只读对比。覆盖双栈 TCP、UDP DNS、正常停止、授权服务复用、端口冲突、桌面断连、内核 SIGKILL、外来策略路由保留、所属规则丢失以及独立的 `ExecStopPost` 清理入口。另覆盖严格 `rp_filter` 拒绝、仅小写 GeoSite 文件的离线启动，以及普通代理、TUN 重启和崩溃后的节点选择持久化。测试只在确认独立的网络命名空间内设置该场景需要的 sysctl。

2026-10-30：上述隔离场景通过；niri 实机的显式代理、已有代理/VPN 冲突拦截、失败保留原内核通过。两组验证的宿主路由/DNS 摘要均未变。当前机器没有安装此服务，也没有开启真实 TUN。

此外，一次性 KVM 虚拟机已补充真实 systemd/polkit、服务权限、自动 `ExecStopPost`、KDE 图形授权的取消与成功，以及外部代理连接验证。图形路径使用 Xorg，未借用宿主登录会话或网络。

**第二阶段的宿主实网验收仍未完成。** CI VM 验收（见上节）覆盖了服务链与 TUN 资源/数据面的自动化验收，但 niri/Wayland 日常桌面宿主仍没有实际开启 TUN；不能把隔离系统、图形检查或 CI VM 当作当前宿主的完整日用或公开发布验收。各轮的版本、范围和结果见 [Linux 测试记录](LINUX-TESTING.md)。
