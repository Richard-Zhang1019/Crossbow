# Crossbow 与 mihomo 内核的交互协议

桌面端通过 YAML 文件和 CLI 参数启动 mihomo。运行时，壳层通过 `127.0.0.1` 上的
HTTP REST（External Controller）和 WebSocket 与内核交换 JSON。实现文件见 §9。

## 1. 交互模型

mihomo 作为独立进程运行。`CoreManager` 管理启动、就绪探测、崩溃重启和停止。
Windows 和 Linux 的 TUN 内核可由平台服务拉起。

| 通道 | 载体 | 位置 | 用途 |
|---|---|---|---|
| 启动配置 | YAML 文件 + CLI 参数 | 普通 sidecar 使用 `<workdir>/config.yaml`，TUN 服务可生成自己的运行配置 | 订阅渲染结果 + 运行时参数注入 |
| 控制面 | HTTP REST（JSON） | `127.0.0.1:<controller_port>` | 模式切换、代理组快照、节点选择、测速、连接管理、配置热重载 |
| 数据面 | WebSocket（JSON） | `127.0.0.1:<controller_port>` | traffic / connections / logs 三路实时推送 |
| 数据平面 | HTTP + SOCKS5 混合代理 | 默认 `127.0.0.1:<mixed_port>`，启用局域网接入时可监听所有接口 | 应用与系统代理的实际流量入口 |

前端 UI 不直连内核：UI → Tauri 命令 → Rust 壳 → 控制 API。WS 数据由 Rust 侧
（`engine/bridge.rs`）节流聚合后经 Tauri 事件（`traffic://data`、`connections://data`、
`logs://data`）转发给页面——连接页 ≥1s 一帧、最多 1000 行，日志 300ms / 20 条批量，
避免大快照刷爆 webview。

## 2. 控制通道与鉴权

- **监听**：`external-controller: 127.0.0.1:<controller_port>`，随 `config.yaml` 注入；
  `controller_port` 为 0 时不注入控制器（测试/无头场景）。
- **端口**：运行时用 `free_port()` 分配（先 `bind 127.0.0.1:0` 拿空闲端口再释放），
  并显式避开 `mixed_port`，防止自相争抢。
- **密钥**：`Uuid::new_v4().simple()`（32 位 hex），随 `secret` 字段写入配置；每次启动链恒定。
- **REST 鉴权**：请求头 `Authorization: Bearer <secret>`。
- **WS 鉴权**：查询参数 `token=<secret>`。[`ws_url`](../apps/desktop/src-tauri/src/engine/bridge.rs)
  对普通路径使用 `?token=`，对已有查询串的路径使用 `&token=`，例如 `/logs?level=info&token=…`。
  `ws_url_includes_token` 覆盖这两种拼接方式。
- **路径编码**：组名/节点名常含中文、空格、emoji，路径段按 RFC 3986 百分号编码
  （字母数字与 `-_.~` 保留），例：`/proxies/%E9%A6%99%E6%B8%AF%20%E8%87%AA%E5%8A%A8`。
- **环境代理隔离**：客户端构造时关闭 env 代理（`try_proxy_from_env(false)`），
  防止回环控制请求被系统代理劫持。

早期的 [总体设计](DESIGN.md#34-各平台系统能力) 规划过 macOS Unix socket 和 Windows 命名管道。
桌面壳访问 mihomo 控制器使用 loopback TCP。Windows 和 Linux 的 TUN 服务另有本地 IPC。

## 3. 协议清单

### 3.1 REST（External Controller）

| 方法 | 路径 | 用途 | 实现入口 |
|---|---|---|---|
| GET | `/version` | 就绪探测 / 诊断 | `Controller::version` |
| GET | `/configs` | 读运行态配置（mode、tun） | `tun_enabled` / `active_tun_device` / `current_outbound` |
| PATCH | `/configs` | 切模式 / TUN 热开关 | `patch_mode` / `patch_tun_enable` |
| PUT | `/configs?force=true` | 从磁盘热重载配置 | `reload_config` |
| GET | `/proxies` | 全量代理与组快照 | `groups` / `current_outbound` |
| GET | `/proxies/{group}` | 单组详情（校验成员、读 fixed） | `select_proxy` / `reset_proxy_selection` |
| PUT | `/proxies/{group}` | 选择组内节点 | `select_proxy` |
| DELETE | `/proxies/{group}` | 清除 URLTest 手动固定 | `reset_proxy_selection` |
| GET | `/group/{group}/delay?url=…&timeout=…` | 组内并发测速 | `test_group_delay` |
| GET | `/proxies/{node}/delay?url=…&timeout=…` | 单节点测速 | `test_node_delay` |
| GET | `/rules` | 规则表（推算当前出口） | `current_outbound` |
| DELETE | `/connections/{id}` | 断开单条连接 | `close_connection` |

### 3.2 WebSocket

| 路径 | 频率 | 用途 |
|---|---|---|
| `/traffic?token=…` | 1 Hz（零流量也推帧） | 实时上下行速率（托盘/首页） |
| `/connections?token=…` | 1 Hz 快照 | 连接页数据（壳层再节流） |
| `/logs?level=info&token=…` | 事件驱动 | 日志页与可选的日志落盘 |

WS 读线程阻塞式读取、断线自动重连；`/traffic`、`/connections` 带心跳，
静默超过阈值（5s）即判定半开并强制重连。`/traffic` 还有一条独立 channel
（`traffic-tray`）专供托盘，与前端订阅互不干扰。

## 4. REST 字段详解

### 4.1 `GET /version`

```json
{ "version": "v1.19.0" }
```

就绪探测轮询此接口直到返回 200（默认总超时 120s：首次启动可能要先下载 GeoIP 数据库）。

### 4.2 `GET /configs`（仅消费以下字段）

| 字段 | 类型 | 含义 |
|---|---|---|
| `mode` | string | 运行模式：`direct` / `rule` / `global` |
| `tun.enable` | bool | TUN 是否启用（关闭时 `tun.device` 不参与判断） |
| `tun.device` | string | 当前 TUN 设备名，macOS 通常为 `utun<数字>`，Windows 和 Linux 服务固定使用 `Crossbow` |

### 4.3 `PATCH /configs`（请求体二选一）

```json
{ "mode": "rule" }
```
- 切换运行模式，仅运行时生效、不落盘（重启回配置文件里的 `mode`）。

```json
{ "tun": { "enable": true } }
```
- macOS 上，运行中的 mihomo 控制器可用时优先尝试热切换。热开启还要求正在使用已授权的内核副本，
  并验证 TUN 接管生效。条件不满足或热切换失败时走重启流程。Windows 和 Linux 通过各自的服务管理 TUN 内核。

### 4.4 `PUT /configs?force=true`

```json
{ "path": "/path/to/config.yaml", "payload": "" }
```

| 字段 | 含义 |
|---|---|
| `path` | 壳层传入本次 `-f` 使用的 `<workdir>/config.yaml`，位于内核工作目录内，无需设置额外的 `SAFE_PATHS` |
| `payload` | 内核保留字段，壳层固定传空串 |

符合热重载条件时，壳层先写盘再发请求（60s 超时）。失败时配置切换流程可回退到重启，
切换最终失败时尝试恢复原配置、内核和系统代理。

### 4.5 `GET /proxies` / `GET /proxies/{group}`

```json
{
  "proxies": {
    "节点选择": { "type": "Selector", "now": "♻️ 自动选择", "all": ["♻️ 自动选择", "香港 01"] },
    "♻️ 自动选择": { "type": "URLTest", "now": "香港 01", "fixed": "", "all": ["香港 01", "SG 01"] },
    "香港 01": { "type": "Shadowsocks", "alive": true, "history": [{ "time": "2026-11-06T10:00:00+08:00", "delay": 128 }] },
    "DIRECT": { "type": "Direct" },
    "REJECT": { "type": "Reject" }
  }
}
```

| 字段 | 出现位置 | 含义 |
|---|---|---|
| `type` | 节点/组 | 类型。Selector / URLTest / Fallback / LoadBalance → 壳层展示为「组」；Direct / Reject / Compatible / Pass → 过滤不展示；其余（Shadowsocks、Trojan、Vmess…）→ 具体节点 |
| `all` | 组 | 组内全部成员名 |
| `now` | 组 | 当前选中（Selector/URLTest 等） |
| `fixed` | 组 | mihomo 扩展字段：URLTest 被手动固定到的节点名；空串 `""` 表示未固定、由内核自动选路。sing-box clash_api **不暴露**该字段，壳层按未固定处理 |
| `alive` | 节点 | 节点存活 |
| `history` | 节点/自动组 | 测速历史数组，壳层只取最后一条的 `delay`（ms，>0 才有效）；`time` 为 RFC3339 |

**选择节点（`PUT /proxies/{group}`）**：请求体 `{"name": "<成员名>"}`。
壳层先 GET 组校验目标确为成员。若来源组为 Selector，且选中的是 `fixed` 非空的 URLTest 子组
（曾被手动固定）→ 先 DELETE 子组清固定 → 再 PUT。保证「切回自动组」恢复内核自动选路。

**恢复自动（`DELETE /proxies/{group}`）**：仅对 `type=URLTest` 且 `fixed` 为非空字符串的组发出请求。
DELETE 同时清理内核选择缓存。`fixed` 为空字符串时，壳层直接返回成功。非自动组或兼容内核未提供 `fixed` 时，壳层返回错误。

### 4.6 测速

- 基准 URL：`http://www.gstatic.com/generate_204`（204 无正文，机场与内核通用）；`timeout=5000`。
- `GET /group/{group}/delay` → `{ "<节点名>": <延迟ms>, ... }`，组内并发探测，失败节点缺席。
- `GET /proxies/{node}/delay` → `{ "delay": <延迟ms> }`；无 `delay` 或为 0 视为失败。

### 4.7 `GET /rules`

```json
{ "rules": [ { "type": "MATCH", "proxy": "节点选择", "payload": "" } ] }
```

| 字段 | 含义 |
|---|---|
| `type` | 规则类型：`MATCH`、`DOMAIN-SUFFIX`、`IP-CIDR`、`GEOIP`… |
| `proxy` | 规则目标：`DIRECT` / `REJECT` / 组名 / 节点名 |
| `payload` | 规则参数（如域名、CIDR） |

规则模式下，壳层取最后一条 MATCH 规则的 `proxy`，沿 `/proxies` 的 `now` 链下钻，
为概览提供兜底出口。sing-box 缺少 MATCH 时可回退到 `route.final`。全局模式以 `GLOBAL` 为起点。
每条实际连接使用的链路由 `/connections` 快照提供。

### 4.8 `DELETE /connections/{id}`

`id` 来自 WS `/connections` 快照中的连接 ID，断开单条连接。

## 5. WebSocket 消息字段详解

### 5.1 `/traffic`

```json
{ "up": 1024, "down": 40960 }
```

`up` / `down`：字节/秒。即使零流量内核也每秒推帧，因此可兼作心跳。

### 5.2 `/connections`

```json
{
  "connections": [
    {
      "id": "c1",
      "upload": 1024,
      "download": 40960,
      "start": "2026-11-06T10:00:00.123456+08:00",
      "chains": ["节点选择", "♻️ 自动选择", "香港 01"],
      "rule": "DomainSuffix",
      "rulePayload": "example.com",
      "metadata": {
        "host": "example.com",
        "destinationIP": "93.184.216.34",
        "destinationPort": "443",
        "network": "tcp",
        "type": "HTTPS",
        "processPath": "/Applications/Curl.app/Contents/MacOS/curl"
      }
    }
  ],
  "uploadTotal": 73728,
  "downloadTotal": 294912,
  "memory": 20971520
}
```

| 字段 | 含义 | 壳层紧凑化 |
|---|---|---|
| `id` | 连接 ID（供 DELETE） | `id` |
| `upload` / `download` | 本连接累计字节 | `up` / `down` |
| `start` | 连接开始时间（RFC3339，可带小数与偏移） | 换算为 `elapsed_ms`（距当前毫秒），缺失、无法解析或晚于当前时间时为 `null` |
| `chains` | 内核报告的决策链 | 保留内核顺序，以 `" → "` 拼接 |
| `rule` / `rulePayload` | 命中的规则类型 / 参数 | 拼接为 `rule`，如 `DomainSuffix example.com` |
| `metadata.host` | 目标域名（缺失时用 `destinationIP`） | `host`，拼接端口为 `host:port` |
| `metadata.destinationIP` / `destinationPort` | 目标 IP / 端口 | 见上 |
| `metadata.network` / `type` | 传输层（tcp/udp）/ 连接类型（HTTP、HTTPS、TCP…） | `network` / `kind` |
| `metadata.process` / `processPath` | 发起进程名称与路径 | `process` 优先用名称，缺失时取路径末段，`process_path` 保留路径 |
| `uploadTotal` / `downloadTotal` | 内核累计上传 / 下载（字节） | `upload_total` / `download_total`，缺失时为 `null` |
| `memory` | 内核报告的内存用量（字节） | `memory`，缺失时为 `null` |

壳层将快照裁剪为上述紧凑结构，单次最多 **1000 行**（超出截断并以 `truncated`
标记原始行数）；连接页按 ≥1s 节流，避免数千行快照拖垮界面。

启用连接进程解析且内核未提供进程名称和路径时，壳层可以按本机 socket 归属补齐。
这只适用于来源与入站地址均为 loopback 的代理连接。macOS 和 Windows 支持 HTTP、HTTPS、
SOCKS4、SOCKS5 的 TCP 连接，Windows 也支持 SOCKS5 UDP。TUN、局域网客户端和查不到归属的连接仍可能没有进程信息。

### 5.3 `/logs?level=info`

```json
{ "type": "info", "payload": "[TCP] 127.0.0.1:44683 --> example.com:443" }
```

| 字段 | 含义 |
|---|---|
| `type` | 日志级别：`debug` / `info` / `warning` / `error`（查询参数 `level` 过滤） |
| `payload` | 日志正文 |

日志通道为进程级单读线程：页面离开后仍缓存（最近若干条），重进时带回缺失条目；
开启日志落盘时，独立订阅会把 `type` 和 `payload` 交给落盘端。启动失败或崩溃摘要来自内核标准输出文件，见 §7。

## 6. 启动配置（YAML）与 CLI

### 6.1 CLI 参数

```bash
mihomo -d <workdir> -f <workdir>/config.yaml
# sing-box 对照：sing-box run -D <workdir> -c <workdir>/config.yaml
```

内核 stdout/stderr 重定向到 `<workdir>/core-stdio.log`（崩溃诊断依据）。
Windows 下附加 `CREATE_NO_WINDOW`，避免 GUI 进程弹控制台。Windows TUN 服务校验输入后，
将运行配置写入按 Windows 用户 SID 隔离的服务目录，再从该目录启动内核。

### 6.2 壳层注入的运行时字段（`crates/crossbow-core/src/runtime.rs`）

| 字段 | 值 | 含义 |
|---|---|---|
| `mixed-port` | 用户设置（默认 7897） | HTTP+SOCKS5 混合端口 |
| `allow-lan` / `bind-address` | false / `"127.0.0.1"`（true 时为 `"*"`） | 是否允许局域网接入 |
| `mode` | `"rule"` | 初始运行模式 |
| `log-level` | `"info"`（可 debug/info/warning/error/silent） | 内核日志级别 |
| `external-controller` | `"127.0.0.1:<controller_port>"` | 控制器监听（仅 loopback） |
| `secret` | 随机 UUID v4 | 控制器访问密钥 |
| `profile.store-selected` | `true` | 内核把手工选择持久化到工作目录（重启不丢） |
| `tun.enable` | true（开启 TUN 时） | mihomo 内置 sing-tun（gVisor）接管全局流量 |
| `tun.stack` | `"gvisor"` | TUN 协议栈 |
| `tun.device` | Windows 注入 `"Crossbow"` | 固定 Wintun 设备名，共存检测据此排除自家适配器 |
| `tun.auto-route` / `auto-redirect` / `auto-detect-interface` | true | 自动接管路由 / NAT / 出口探测 |
| `tun.inet6-address` | `["fdfe:dcba:9876::1/126"]` | TUN IPv6 地址（存在原生 v6 时注入） |
| `tun.inet6-route-address` | `["::/1", "8000::/1"]` | IPv6 分流路由 |
| `tun.inet6-route-exclude-address` | `["fe80::/10", "fc00::/7", "ff00::/8"]` | 排除链路本地 / ULA / 组播 |
| `tun.route-exclude-address` | `["192.168.0.0/16", "100.64.0.0/10", "10.0.0.0/8", "172.16.0.0/12"]` | 私网 / CGNAT（Tailscale）段豁免 |
| `tun.dns-mode` | `"native"` | 系统 DNS 指向 TUN 地址 |
| `tun.dns-hijack` | `["any:53", "tcp://any:53"]` | 捕获标准 DNS 请求 |
| `dns.enable` / `dns.enhanced-mode` / `dns.ipv6` | true / `"fake-ip"` / 随原生 v6 | TUN 必需；AAAA 默认过滤 |
| `sniffer` | TUN 启用时注入。TLS `443`、`8443`，HTTP `80`、`8080-8880`。`enable`、`override-destination`、`force-dns-mapping`、`parse-pure-ip` 为 `true`，HTTP 也显式开启 `override-destination` | 从 TLS SNI 或 HTTP Host 恢复域名，供域名规则匹配与目标重写 |

Linux 服务会重写 TUN 配置，固定设备名为 `Crossbow` 并关闭内核自动路由和自动重定向，由服务管理路由。
DNS 和共存模式见 [Linux 网络接管](LINUX-NETWORK.md)。

`rules` 顶部固定前置 4 条直连（先于一切订阅规则命中）：

```
IP-CIDR,100.64.0.0/10,DIRECT,no-resolve
IP-CIDR,10.0.0.0/8,DIRECT,no-resolve
IP-CIDR,172.16.0.0/12,DIRECT,no-resolve
IP-CIDR,192.168.0.0/16,DIRECT,no-resolve
```

## 7. 生命周期与健壮性

- **就绪探测**：轮询 `GET /version`（Bearer），默认 120s 总超时；首次启动含 GeoIP
  数据库下载（应用侧镜像预下载失败时内核直连 GitHub 最慢，故窗口要长）。
- **崩溃自动重启**：直接管理的子进程在 60s 窗口内最多自动重启 3 次，每次等待 800ms。
  崩溃状态使用 `core-stdio.log` 中最后一条错误或致命日志的脱敏摘要，超限后保留 `Crashed`。
- **停止与重启**：Unix 上优先发送 `SIGTERM`，给内核 5s 执行退出清理（包括 TUN 路由）。
  仍未退出时使用 `SIGKILL`，再等待最多 2s 确认。macOS 停止授权内核遇到权限错误时可请求管理员权限，
  发信号前复核进程路径和启动时间。若不能确认退出，保留进程追踪和描述符，阻止拉起新内核。
  服务托管的进程优先通过服务执行停止和资源清理。
- **轻量交接描述符**：GUI 退出/自动更新前把 `{port, secret, pid, binary_path, config}`
  原子写入 `<workdir>/core-descriptor.json`（Unix 权限 0600，临时文件 + rename）；
  下一实例检查 PID 与二进制身份，并通过记录的端口和密钥探测 `/version`。
  确认原进程已退出、身份不再匹配或安全清理成功后才删除描述符。
  Windows 服务托管时，描述符记录服务安装目录钉定的 `mihomo.exe`。托管内核的就绪结果只有在 PID、控制器端口和密钥仍与当前启动批次一致时才生效。
  Linux TUN 服务随拥有租约的 GUI 连接释放资源，不参与普通的内核交接。
- **端口清理**：启动前检查混合端口。尝试清理残留进程时，须核实二进制完整路径等进程身份信息。
  Windows 的监听端口和进程镜像名可用于找候选进程。macOS 必要时通过管理员授权清理。

## 8. 与其他引擎 / 端的差异

- **sing-box（桌面）**：通过 `experimental.clash_api` 复用 REST 客户端和 WS 桥。已知差异包括以下三项。
  - `fixed` 字段不被 clash_api 暴露（按未固定处理，恢复自动不发 DELETE）；
  - `PATCH /configs` 的 TUN 热更新不可用（仅 mihomo）；
  - `/rules` 无合成的 `MATCH` 规则（当前出口回退读配置 `route.final`）。
- **iOS**：进程内 libbox（sing-box 官方移动库），**没有控制 socket**；
  壳层通过 `libbox.CommandServer` 的 `StartOrReloadService` / `CloseService` 启停，
  TUN 参数经 `Platform.ApplyNetworkSettings` 传给系统扩展。

## 9. 参考实现

| 关注点 | 文件 |
|---|---|
| REST 客户端（接口、路径编码、契约测试） | [`engine/api.rs`](../apps/desktop/src-tauri/src/engine/api.rs) |
| WS 桥（订阅、节流聚合、日志缓存、重连） | [`engine/bridge.rs`](../apps/desktop/src-tauri/src/engine/bridge.rs) |
| 内核生命周期与配置切换 | [`engine/manager.rs`](../apps/desktop/src-tauri/src/engine/manager.rs)、[`engine/flow.rs`](../apps/desktop/src-tauri/src/engine/flow.rs) |
| 运行时配置注入（YAML 合并、CLI 差异） | [`runtime.rs`](../crates/crossbow-core/src/runtime.rs) |
| 内核下载与版本锁定 | [`engine/download.rs`](../apps/desktop/src-tauri/src/engine/download.rs) |
| 平台 TUN 切换 | [macOS](../apps/desktop/src-tauri/src/network/tun.rs)、[Windows](../apps/desktop/src-tauri/src/network/tun_windows.rs)、[Linux](../apps/desktop/src-tauri/src/network/tun_linux.rs) |
| Windows 服务托管与固定内核路径 | [`service_client.rs`](../apps/desktop/src-tauri/src/service_client.rs)、[`apps/service/src/windows.rs`](../apps/service/src/windows.rs) |
| 前端命令（UI → Tauri） | [`commands/proxies.rs`](../apps/desktop/src-tauri/src/commands/proxies.rs)、[`commands/monitor.rs`](../apps/desktop/src-tauri/src/commands/monitor.rs)、[`commands/core.rs`](../apps/desktop/src-tauri/src/commands/core.rs) |
| iOS 内嵌引擎 | [`TunnelCore/tunnelcore.go`](../apps/ios/TunnelCore/tunnelcore.go) |
