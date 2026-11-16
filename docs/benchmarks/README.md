# 网络性能基线（Refs #492）

本目录先提供 **mihomo 显式 HTTP 代理**的隔离基线。使用 DIRECT-only 脱敏夹具、
私有运行目录、随机端口和一次性控制器密钥，不读取用户订阅，不修改系统代理、
DNS、TUN 或路由。它尚不是系统代理接管或 TUN 的性能验收，#492 保持开放。

## 复跑

```sh
# 默认仅访问回环上游：两轮，每轮 30 个延迟样本及至少 1 秒吞吐负载。
python3 scripts/benchmark/bench_network.py \
  --core apps/desktop/src-tauri/binaries/mihomo-aarch64-apple-darwin \
  --mode explicit-proxy --output target/network-benchmark

# 可选的公网 HTTPS 与系统解析器观测；按目标分别统计，失败也保留。
python3 scripts/benchmark/bench_network.py --core <mihomo-binary> \
  --public-samples 1 --dns-samples 3 --timeout 5 \
  --output target/network-benchmark
```

`--mode sysproxy` 仅是旧命令的兼容别名，结果仍标为 `explicit-proxy`。
`--mode tun` 一律拒绝，包括携带旧的放行参数时；混合端口请求不能证明 TUN
转发，回环流量也不适合作为 TUN 路径证据。真实系统代理、TUN、sing-box 和
Windows 基线待对应实现与受控环境验证。Windows 未提供 `ps` 时资源指标不可用。

每次输出独立 JSON 文件，默认写到 `target/network-benchmark`，不覆盖已归档结果。
密钥仅用于临时夹具，归档中脱敏。记录代码提交、脚本/内核 SHA-256、版本、
环境、参数和夹具；`worktree_dirty` 为真时必须结合脚本哈希解释可复现范围。

## 路径证据与失败处理

- HTTP 用 absolute-form 连接 `127.0.0.1:<mixed-port>`；HTTPS 用 CONNECT 和
  Python 默认信任库验证证书。绕过 `NO_PROXY`、环境代理和 macOS 代理豁免，
  不自动跟随重定向。负对照要求关闭的代理端口访问回环失败。
- 每个有效样本还需匹配本次所拥有内核日志里的源地址/端口与 DIRECT 连接，
  保存内核报告的目的端。收到响应而缺少证据仍算失败。
- `core_outbound_intent: DIRECT` 是夹具意图，日志只验证内核出口。物理出口
  始终标为未验证：其他 VPN、宿主路由或 NAT 可能影响公网路径。
- 记录客户端默认 CA 的可用状态、启动时加载的 CA 数量及 TLS 验证错误码。
  加载数为零也可能是延迟加载，需结合文件/目录可用性和实际验证错误解释；
  不能把客户端信任配置问题直接判为内核或线路故障，不关闭证书校验以凑成功率。
- 延迟请求必须返回 204，吞吐必须返回 200 且实收字节数完全匹配；错误状态、
  重定向、TLS/连接/超时、短正文和缺失路径证据分别记录。
- 启动就绪失败、测量异常和重启异常均回收本脚本创建的子进程，超时升级 kill
  并 wait；失败也输出结果及清理状态。不会根据端口查找并终止其他进程。

## schema 2 指标

| 字段 | 定义与限制 |
| --- | --- |
| `cold_start` | 独立内核 spawn 前到控制器就绪、首个成功代理 204 的时间；不含应用启动。20ms 就绪轮询会影响分辨率。响应计时不含后续日志核验等待。 |
| `rounds[].local_http_204` | 已启动内核的回环 HTTP 延迟；每次新建连接，不等于 DNS/TLS 热缓存。保留每个成功/失败样本。 |
| `p50_ms` / `p95_ms` | 仅成功样本的 nearest-rank 百分位；浮点毫秒，不截断为整数。空成功集合为 null。 |
| `public_https` | 按 URL 分开统计的公网 HTTPS 204。含连接、TLS 和请求开销，不混排不同目标；默认未运行。 |
| `throughput` | 回环 HTTP 经内核的端到端有效吞吐，MiB/s；按实收完整字节/实际耗时计算，包含客户端/上游/连接与路径核验开销，不能单独解释为内核性能或公网速度。 |
| `throughput.resources` | 仅吞吐阶段采样窗口；由子进程累计 CPU 时间差计算单核百分比，可能超过 100%。峰值是采样间隔平均峰值，RSS 是采样最大值，不是全程绝对峰值。记录窗口与点数，受 `ps TIME` 平台精度及采样开销影响。 |
| `system_resolver` | 独立 worker 内 `getaddrinfo` 耗时，排除 worker 启动开销。缓存、物理出口未控制，不能称为直连物理 DNS 或内核 DNS；默认未运行，失败/超时分别保留。 |
| `controlled_process_restart` | terminate 旧进程到新进程首个代理 204，含停止等待、spawn 和就绪；另列停止耗时。不是断网、睡眠唤醒或 Crossbow 自动恢复时间。 |
| `cleanup` | 所有本脚本拥有的内核子进程是否已回收。 |

`status: completed` 表示测量流程和必需的本地路径验证完成；不代表可选公网/DNS
样本全部成功。查看 `probe_failures`、各组成功率与错误分类。不能从一个 DIRECT
回环基线推导节点协议、TUN 或弱网下的默认值。

## schema 2 原生记录（2026-11-14）

[显式代理 v2 JSON](2026-11-14-explicit-proxy-v2.json) 来自干净提交 `5c1488e`，
macOS 27.2 / Apple M2（8 逻辑 CPU、16 GiB）/ Python 3.14.7 / mihomo v1.19.32。
通过关闭代理负对照、实际代理 peer、逐连接 DIRECT 日志和实收字节数验证。

| 轮次 | 回环 204 | P50 / P95（ms） | 回环端到端吞吐（MiB/s） |
| --- | --- | --- | --- |
| 1 | 30/30 | 0.553 / 0.651 | 3058.589 |
| 2 | 30/30 | 0.348 / 0.548 | 3000.564 |

吞吐每次 64 MiB、每轮至少 1 秒；冷启动和受控进程重启各测一次。公网四次
HTTPS 样本均为证书验证错误码 20，客户端启动时 CA 加载数为 0、默认 CA 文件与
目录均不可用，因此没有有效公网 204 延迟，不能判断线路或内核网络性能。
系统解析器观测 3/3 成功，路径及缓存仍未控制。子进程全部回收；以上仅支持
本次回环工作负载，不支持 TUN、真实系统代理接管或默认值调参结论。

复跑本份参数（保留完整证书验证）：

```sh
NO_PROXY='*' no_proxy='*' python3 scripts/benchmark/bench_network.py \
  --core apps/desktop/src-tauri/binaries/mihomo-aarch64-apple-darwin \
  --mode explicit-proxy --samples 30 --throughput-mib 64 --rounds 2 \
  --load-seconds 1 --public-samples 1 --dns-samples 3 --timeout 5 \
  --output target/network-benchmark
```

## schema 2 公网样本记录（2026-11-16，Refs #503）

[`2026-11-16-explicit-proxy-v2-public-https.json`](2026-11-16-explicit-proxy-v2-public-https.json)
来自干净提交 `2590924`（worktree_dirty=false），macOS 27.2 / Apple M2（8 逻辑
CPU、16 GiB）/ Python 3.14.7 / mihomo v1.19.32。与 2026-11-14 记录唯一的客户端
差异：以 `SSL_CERT_FILE=/etc/ssl/cert.pem` 指定 macOS 系统 CA 包（启动加载
128 个证书，`default_ca_file_available=true`），证书与主机名校验全程开启；
2026-11-14 的四次错误码 20 样本仍按客户端信任配置缺失口径保留，不与本份数字
互比线路质量。

| 轮次 | 回环 204 | 回环 P50 / P95（ms） | 回环端到端吞吐（MiB/s） | gstatic 204 | cloudflare 204 |
| --- | --- | --- | --- | --- | --- |
| 1 | 30/30 | 0.531 / 1.831 | 1843.553 | 2/2，155.4 / 205.1 | 2/2，629.1 / 665.6 |
| 2 | 30/30 | 0.378 / 0.672 | 1668.076 | 2/2，127.7 / 222.6 | 2/2，700.6 / 708.3 |

- 复跑命令：同上块，改用 `--throughput-mib 8 --public-samples 2` 并前置
  `SSL_CERT_FILE=/etc/ssl/cert.pem`。脚本哈希 `5a4c5af8…`，内核 SHA-256 记录
  在 JSON 内；负对照（代理端口关闭访问回环）按预期连接失败。
- 冷启动 30.5 ms、受控进程重启 33.7 ms（terminate→首个代理 204）；系统解析器
  3/3 成功（P50 3.3 ms，缓存与物理出口未控制）；子进程全部回收。
- 公网样本逐条通过内核日志 DIRECT 路径核验；物理出口仍标未验证（宿主 VPN/
  路由可能影响公网路径）。数字仅代表本次 DIRECT 显式代理负载，不支持 TUN、
  真实系统代理接管、弱网或默认值调参结论——见「静态审计与待验证边界」。

## 双内核同基线对比（2026-11-16，Refs #503）

`bench_network.py` 新增 `--engine sing-box`：按引擎生成 DIRECT-only 夹具配置
（sing-box JSON：mixed 入站 + clash_api 控制器 + direct 出站）、启动命令、
版本探测与逐样本日志证据解析（sing-box 以连接 id 关联 ANSI 着色行）。同
commit `30463fb`、同脚本哈希、同参数（30 样本 × 2 轮、吞吐 8 MiB、公网
2/目标、DNS 3），`worktree_dirty=false`：

| 内核 | 回环 204 | 回环 P50 / P95（ms） | 吞吐（MiB/s） | 冷启动（ms） | 重启（ms） |
| --- | --- | --- | --- | --- | --- |
| [mihomo v1.19.32](2026-11-16-mihomo-dual-engine.json) | 30/30 × 2 | 0.561 / 0.709；0.345 / 0.626 | 2204.7；2142.1 | 30.1 | 29.6 |
| [sing-box 1.14.2](2026-11-16-singbox-dual-engine.json) | 30/30 × 2 | 0.530 / 0.724；0.538 / 0.755 | 1959.8；1351.8 | 55.8 | 84.1 |

两内核公网样本全部 204 且逐条通过各自内核日志的 DIRECT 证据核验；
sing-box 控制器经 clash_api 与 mihomo 同形。差异提示（非结论）：sing-box
冷启动/重启更慢（进程语义不同）、吞吐轮间波动更大；数字仅代表本次回环
DIRECT 负载，不能外推到节点协议或真实链路性能。

## 受控弱网矩阵（2026-11-16，Refs #503）

[2026-11-16-weak-network-matrix.json](2026-11-16-weak-network-matrix.json)
（commit `30463fb`，dirty=false）。新增
`scripts/benchmark/bench_weak_network.py`：toxiproxy 施加在「客户端 → 显式
代理」链路上的受控损伤（内核 DIRECT 路径由独立 sanity 样本归因证明；经
toxiproxy 的逐样本日志归因不可用——内核侧连接源是 toxiproxy 出口端口）。

| 场景 | toxics | 回环 204 | P50 / P95（ms） | 吞吐（MiB/s） |
| --- | --- | --- | --- | --- |
| baseline | 无 | 20/20 | 0.705 / — | 1048.3 |
| latency_50ms（双向） | latency 50ms j10 | 20/20 | 105.8 / 117.1 | — |
| latency_150ms（双向） | latency 150ms j30 | 20/20 | 318.1 / 356.2 | — |
| bandwidth_512k | 带宽 512 KB/s 下行 | 10/10 | 1.312 / — | **0.488** |
| intermittent | latency 200ms j50 + timeout 500ms（toxicity 0.5） | **11/20** | 243.3 / 251.1 | — |
| sustained_slow | latency 300ms j30 | 20/20 | 326.2 / 332.3 | — |
| outage_recovery | listen 关闭 2s 后重开 | — | 恢复 1.2ms（首样本） | — |

**偶发与持续的指标区分**（#503 验收要求）：intermittent 成功率 55%（≈
toxicity 0.5）但 ok 样本快（p50 243ms）；sustained_slow 成功率 100% 但全体
慢（p50 326ms）——「成功率」与「延迟分位数」必须成对报告，单看任一指标都
会误判链路健康度。恢复时间与受控重启分别记录。

**指标不可混用夹具**：同一夹具内，TCP 可达（accept 不应答）而代理 HTTP
超时；HTTP 204 正常而对上游无 UDP 监听端口发数据报超时——TCP 可达性、
代理 HTTP 延迟、UDP 可用性三者互相不能替代，节点测速指标必须按协议族
分别报告。

复跑命令：

```sh
python3 scripts/benchmark/bench_weak_network.py \
  --core apps/desktop/src-tauri/binaries/mihomo-aarch64-apple-darwin \
  --toxiproxy-bin /opt/homebrew/bin/toxiproxy-server --samples 20 \
  --output target/network-benchmark
python3 scripts/benchmark/bench_network.py \
  --core <core-binary> --engine <mihomo|sing-box> \
  --samples 30 --throughput-mib 8 --rounds 2 --public-samples 2 --dns-samples 3 \
  --output target/network-benchmark
```

弱网未覆盖（待后续）：真丢包模型（toxiproxy 无丢包 toxic，需 netem/容器）、
UDP 受限链路、IPv6 异常、公网目标弱网。

## TUN 路径观测证据（2026-11-16，Refs #502/#503）

bench 脚本继续拒绝 TUN 模式（混合端口探针不能证明 TUN 转发）。以下为
**观测性证据**（应用实机验收期间记录，非基准数据，无吞吐指标）：

- TUN 内核：`tun-binaries/` 独立目录的 mihomo（v1.19.32），单实例同时服务
  mixed-port 与 TUN 入站。
- 接口与路由：`utun6`（198.18.0.1）UP；default 路由仍在物理网关，mihomo 以
  细分前缀路由（1、2/7、4/6、8/7…共 41 条 → utun6）覆盖 IPv4 空间。
- 非回环目标连通：TUN 劫持路径 curl 204（0.34–0.78s，热点/Wi-Fi 均测）。
- 切网轨迹（2 秒采样）：热点↔家 Wi-Fi 双向切换中 41 条劫持路由**全程未被
  清除**，default 平滑迁移（切换在两采样间隙内完成）；双接管下系统代理
  7898 无漂移；全程 decision.log 零新行（零重启）。
- 拆除：关闭 TUN 后 utun 接口与全部劫持路由移除，内核进程复用转显式代理
  模式（零中断）。

## 旧数据

[`2026-11-14-sysproxy-baseline.json`](2026-11-14-sysproxy-baseline.json) 保留原始数值，
但已标记无效：原方法在 `NO_PROXY=*` 且代理端口关闭时仍能把回环请求计为成功，
缺少内核路径证据；CPU/恢复时间等口径也不符合当前定义。不能用它证明内核转发
性能，也不能与 schema 2 数值作优化前后对比。

## 静态审计与待验证边界

以下仅审阅当前源代码，未证明实际 DNS/路由路径：

- [GUI mihomo 运行注入](../../crates/crossbow-core/src/runtime.rs) 在 TUN 开启时
  写入 gVisor、自动路由/网卡检测、native DNS、fake-ip 和嗅探配置；IPv6 注入
  受原生 IPv6 状态门控，私网/CGNAT 及 IPv6 本地范围有排除规则。
  Linux 服务接管另见 [Linux 网络说明](../LINUX-NETWORK.md)，不能从 GUI 注入
  推广到全部平台。
- 运行注入没有强制写入 fake-ip-filter。最终行为还受订阅、合并和内核默认值
  影响，不能假定企业内网后缀必然被过滤；需检查最终配置并实际验证。
- [sing-box 转换](../../crates/crossbow-core/src/singbox_convert.rs) 将节点域名
  引导解析指向 local-dns，proxy-dns 使用 IP 形式 DoH 并按最终出口设置 detour。
  该默认依赖图中没有明显的域名自举环，但这不保证 UDP 上游、宿主路由、
  其他 VPN 或自定义订阅环境中的可用性。规则集下载路径同样依赖最终出口。

仍待覆盖：真实系统代理/TUN、DNS 上游抓包、IPv4-only/双栈/IPv6 异常、受限
UDP、弱网/丢包/MTU、门户、局域网域名/设备、其他 VPN、真实节点/协议和
Windows/sing-box。每轮只变一个变量；同设备、线路、目标和负载才可比较。
默认值修改须引用可复现的对照与回归证据，另提 issue/PR。
