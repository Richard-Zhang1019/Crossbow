# Crossbow 与 mihomo 内核的交互协议

> 本文对应**当前实现**（桌面端 `apps/desktop/src-tauri` + `crates/crossbow-core`），不是规划稿；
> 涉及文件索引见 §9。核心结论：壳层与 mihomo 之间没有进程内调用，全部是**本地协议通信**——
> 启动时通过 YAML 配置 + CLI 参数，运行时通过 `127.0.0.1` 上的
> **HTTP REST（External Controller）** 与 **WebSocket**，载荷均为 JSON。

## 1. 交互模型

mihomo 以 sidecar 子进程运行（`CoreManager` 负责启动、就绪探测、崩溃重启、停止），
壳层（Tauri/Rust）通过四条通道与它打交道：

| 通道 | 载体 | 位置 | 用途 |
|---|---|---|---|
| 启动配置 | YAML 文件 + CLI 参数 | `<workdir>/config.yaml`，`mihomo -d <workdir> -f <config.yaml>` | 订阅渲染结果 + 运行时参数注入 |
| 控制面 | HTTP REST（JSON） | `127.0.0.1:<controller_port>` | 模式切换、代理组快照、节点选择、测速、连接管理、配置热重载 |
| 数据面 | WebSocket（JSON） | `127.0.0.1:<controller_port>` | traffic / connections / logs 三路实时推送 |
| 数据平面 | HTTP + SOCKS5 混合代理 | `127.0.0.1:<mixed_port>` | 应用与系统代理的实际流量入口 |

前端 UI 不直连内核：UI → Tauri 命令 → Rust 壳 → 控制 API。WS 数据由 Rust 侧
（`ws_bridge.rs`）节流聚合后经 Tauri 事件（`traffic://data`、`connections://data`、
`logs://data`）转发给页面——连接页 ≥1s 一帧、最多 1000 行，日志 300ms / 20 条批量，
避免大快照刷爆 webview。

## 2. 控制通道与鉴权

- **监听**：`external-controller: 127.0.0.1:<controller_port>`，随 `config.yaml` 注入；
  `controller_port` 为 0 时不注入控制器（测试/无头场景）。
- **端口**：运行时用 `free_port()` 分配（先 `bind 127.0.0.1:0` 拿空闲端口再释放），
  并显式避开 `mixed_port`，防止自相争抢。
- **密钥**：`Uuid::new_v4().simple()`（32 位 hex），随 `secret` 字段写入配置；每次启动链恒定。
- **REST 鉴权**：请求头 `Authorization: Bearer <secret>`。
- **WS 鉴权**：查询参数 `token=<secret>`；路径自带 query 时（如 `/logs?level=info`）
  必须以 `&token=` 续接，否则 token 被吞进上一个参数导致内核 401、通道无限重连。
- **路径编码**：组名/节点名常含中文、空格、emoji，路径段按 RFC 3986 百分号编码
  （字母数字与 `-_.~` 保留），例：`/proxies/%E9%A6%99%E6%B8%AF%20%E8%87%AA%E5%8A%A8`。
- **环境代理隔离**：客户端构造时关闭 env 代理（`try_proxy_from_env(false)`），
  防止回环控制请求被系统代理劫持。

> 说明：`docs/DESIGN.md` §5.2 曾规划 macOS uds / Windows named pipe 控制通道；
> 当前实现为 loopback TCP + mihomo 内置 HTTP 控制器（收紧计划见 `runtime.rs` 注释）。

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
| `/logs?level=info&token=…` | 事件驱动 | 日志页 + 落盘 + 崩溃摘要 |

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
| `tun.device` | string | 当前 TUN 设备名：macOS/Linux 为 `utun<数字>`；Windows 固定 `Crossbow`（用于共存检测时识别自家适配器） |

### 4.3 `PATCH /configs`（请求体二选一）

```json
{ "mode": "rule" }
```
- 切换运行模式，仅运行时生效、不落盘（重启回配置文件里的 `mode`）。

```json
{ "tun": { "enable": true } }
```
- TUN 热开关：mihomo 运行时重建 TUN 栈，内核不重启；
  用来绕开「杀 root 内核 → 端口交接 → 重启探测」的长链路。
  **仅 mihomo 支持**，sing-box 的 clash_api 不支持热更新。

### 4.4 `PUT /configs?force=true`

```json
{ "path": "/path/to/config.yaml", "payload": "" }
```

| 字段 | 含义 |
|---|---|
| `path` | 重载的配置文件路径；必须是本次 `-f` 加载的那份文件（mihomo 只允许重载自己加载的配置，无需额外 `SAFE_PATHS`） |
| `payload` | 内核保留字段，壳层固定传空串 |

壳层先写盘再发请求（60s 超时），实现「换个配置但不重启进程」。

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
壳层流程：先 GET 组校验目标确为成员 → 若选中的是 URLTest 子组且其 `fixed` 非空
（曾被手动固定）→ 先 DELETE 子组清固定 → 再 PUT。保证「切回自动组」恢复内核自动选路。

**恢复自动（`DELETE /proxies/{group}`）**：仅对 `type=URLTest` 且 `fixed` 为字符串的组有效；
内核同时清理选择缓存（等价 mihomo 面板的「恢复自动选择」）。
**不能**用 `PUT {"name": ""}` 代替；非自动组或内核不支持时壳层不发请求。

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

壳层取**最后一条 MATCH 规则**的 `proxy` 作为兜底出口，沿 `/proxies` 的 `now` 链
逐层下钻到具体节点，得到「当前出口」展示（sing-box 不合成 MATCH，此时回退读配置
`route.final`）。

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
| `start` | 连接开始时间（RFC3339，带小数与偏移） | 换算为 `elapsed_ms`（距当前毫秒） |
| `chains` | 决策链：规则目标组 → … → 最终出口 | 数组 join 为 `" → "` |
| `rule` / `rulePayload` | 命中的规则类型 / 参数 | 拼接为 `rule`，如 `DomainSuffix example.com` |
| `metadata.host` | 目标域名（缺失时用 `destinationIP`） | `host`，拼接端口为 `host:port` |
| `metadata.destinationIP` / `destinationPort` | 目标 IP / 端口 | 见上 |
| `metadata.network` / `type` | 传输层（tcp/udp）/ 连接类型（HTTP、HTTPS、TCP…） | `network` / `kind` |
| `metadata.processPath` | 发起进程路径 | `process`（取文件名）/ `process_path` |
| `uploadTotal` / `downloadTotal` | 会话累计上下行 | `upload_total` / `download_total` |
| `memory` | 内核内存占用（字节） | `memory` |

壳层将快照裁剪为上述紧凑结构，单次最多 **1000 行**（超出截断并以 `truncated`
标记原始行数）；连接页按 ≥1s 节流，避免数千行快照拖垮界面。

### 5.3 `/logs?level=info`

```json
{ "type": "info", "payload": "[TCP] 127.0.0.1:44683 --> example.com:443" }
```

| 字段 | 含义 |
|---|---|
| `type` | 日志级别：`debug` / `info` / `warning` / `error`（查询参数 `level` 过滤） |
| `payload` | 日志正文 |

日志通道为进程级单读线程：页面离开后仍缓存（最近若干条），重进时带回缺失条目；
另有独立落盘通道把 `type` + `payload` 写入日志文件；内核崩溃时取 `error` 行作为
失败摘要（脱敏后上报）。

## 6. 启动配置（YAML）与 CLI

### 6.1 CLI 参数

```bash
mihomo -d <workdir> -f <workdir>/config.yaml
# sing-box 对照：sing-box run -D <workdir> -c <workdir>/config.yaml
```

内核 stdout/stderr 重定向到 `<workdir>/core-stdio.log`（崩溃诊断依据）。
Windows 下附加 `CREATE_NO_WINDOW`，避免 GUI 进程弹控制台。

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
| `tun.device` | `"Crossbow"`（仅 Windows） | 固定 wintun 设备名，共存检测据此排除自家适配器 |
| `tun.auto-route` / `auto-redirect` / `auto-detect-interface` | true | 自动接管路由 / NAT / 出口探测 |
| `tun.inet6-address` | `["fdfe:dcba:9876::1/126"]` | TUN IPv6 地址（存在原生 v6 时注入） |
| `tun.inet6-route-address` | `["::/1", "8000::/1"]` | IPv6 分流路由 |
| `tun.inet6-route-exclude-address` | `["fe80::/10", "fc00::/7", "ff00::/8"]` | 排除链路本地 / ULA / 组播 |
| `tun.route-exclude-address` | `["192.168.0.0/16", "100.64.0.0/10", "10.0.0.0/8", "172.16.0.0/12"]` | 私网 / CGNAT（Tailscale）段豁免 |
| `tun.dns-mode` | `"native"` | 系统 DNS 指向 TUN 地址 |
| `tun.dns-hijack` | `["any:53", "tcp://any:53"]` | 捕获标准 DNS 请求 |
| `dns.enable` / `dns.enhanced-mode` / `dns.ipv6` | true / `"fake-ip"` / 随原生 v6 | TUN 必需；AAAA 默认过滤 |

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
- **崩溃自动重启**：60s 窗口内最多 3 次、退避 800ms；超限保持 `Crashed` 并上报
  `core-stdio.log` 中的最后一条 error（脱敏）。
- **热切换**：模式与 TUN 均 `PATCH /configs`，不重启内核。
- **配置热重载**：先写盘再 `PUT /configs?force=true`，不重启。
- **轻量交接描述符**：GUI 退出/自动更新前把 `{port, secret, pid, binary_path, config}`
  原子写入 `<workdir>/core-descriptor.json`（Unix 权限 0600，临时文件 + rename）；
  下一实例启动时探测 `/version`，通过则「收养」该内核（PID 存活 + 二进制身份校验
  防 PID 复用），失败则删除描述符并杀残留。
- **端口清理**：启动前检测混合端口占用，清理本应用残留内核（Mac/Linux 按二进制路径
  匹配PID，Windows 按 netstat 镜像名），必要时提权。

## 8. 与其他引擎 / 端的差异

- **sing-box（桌面）**：走 `experimental.clash_api`，与 mihomo **同一套 REST/WS 契约**
  （同一个 `mihomo_api::Controller`）。已知差异在代码里做了兼容分支：
  - `fixed` 字段不被 clash_api 暴露（按未固定处理，恢复自动不发 DELETE）；
  - `PATCH /configs` 的 TUN 热更新不可用（仅 mihomo）；
  - `/rules` 无合成的 `MATCH` 规则（当前出口回退读配置 `route.final`）。
- **iOS**：进程内 libbox（sing-box 官方移动库），**没有控制 socket**；
  壳层通过 `libbox.CommandServer` 的 `StartOrReloadService` / `CloseService` 启停，
  TUN 参数经 `Platform.ApplyNetworkSettings` 传给系统扩展。

## 9. 参考实现

| 关注点 | 文件 |
|---|---|
| REST 客户端（全部接口、路径编码、契约测试） | `apps/desktop/src-tauri/src/mihomo_api.rs` |
| WS 桥（订阅、节流聚合、日志缓存、重连） | `apps/desktop/src-tauri/src/ws_bridge.rs` |
| 内核生命周期（端口/密钥、就绪、重启、描述符） | `apps/desktop/src-tauri/src/core_manager.rs` |
| 运行时配置注入（YAML 合并、CLI 差异） | `crates/crossbow-core/src/runtime.rs` |
| 内核下载与版本锁定 | `apps/desktop/src-tauri/src/core_download.rs` |
| 前端命令表面（UI → Tauri） | `apps/desktop/src-tauri/src/lib.rs`（`proxies_snapshot`、`select_proxy`、`test_*_delay`、`current_outbound`、`close_connection`、`set_core_mode`…） |
| iOS 内嵌引擎 | `apps/ios/TunnelCore/tunnelcore.go` |
