# 网络性能基线（#492）

目的：为 DNS、协议与 TUN 的默认值讨论提供**可复现证据**。每轮实验只变一个
变量；默认值变更以收益、回归结果和适用范围为依据，按证据单独提项。

## 运行

```sh
# 系统代理模式（显式经混合端口，不改系统设置；本机可跑）
python3 scripts/benchmark/bench_network.py --core <mihomo 二进制> --mode sysproxy

# TUN 模式：生成对齐 runtime.rs 注入形状的夹具并打印受控环境命令；
# 会建 utun/改路由，默认拒绝在本机执行（--i-know-this-touches-my-network 放行）
python3 scripts/benchmark/bench_network.py --core <mihomo 二进制> --mode tun \
  --i-know-this-touches-my-network
```

输出 `<output>/<mode>-results.json`。夹具内嵌在脚本中（DIRECT-only、无节点
无订阅），端口随机、密钥固定为 benchmark 专用串。

## 指标定义

| 指标 | 定义 |
| --- | --- |
| `cold_first_request_ms` | 进程拉起 → 控制器就绪（`controller_ready_ms`）→ 首个经代理 204 成功（`first_proxied_ok_ms`），单次 |
| `rounds[].p50/p95_ms` | 回环上游 204 的延迟样本（默认 30 个）百分位 |
| `rounds[].latency_ok` | 成功样本数 / 总数（成功率） |
| `rounds[].public_p50_ms` | 经内核 DIRECT 访问公网 204 的真实路径样本（10 个） |
| `rounds[].throughput_mb_s` | 回环 8MB 体经内核下载——隔离 WAN 抖动，度量内核转发开销 |
| `cpu_*` / `rss_peak_kb` | 吞吐阶段内核进程 CPU（均值/峰值）与内存峰值 |
| `dns_direct.p50/p95_ms` | 系统解析器解析基准域名（10 个）——物理 DNS 基线 |
| `recovery_ms` | 杀内核 → 重启 → 首个经代理 204 成功（含拉起，对齐 #490 恢复语义） |

出口与目的端全程记录在结果的 `targets`/`egress` 字段；回环与公网样本分开，
不混排。错误分类沿用 #488（ok/timeout/connect/…）。

## 已记录基线

### 2026-11-14 · macOS 系统代理 · mihomo · DIRECT 出口

环境：`macOS 27.2 arm64`，mihomo v1.19.32，夹具 DIRECT-only，回环上游 +
公网 204；数据 [`2026-11-14-sysproxy-baseline.json`](2026-11-14-sysproxy-baseline.json)。

| 指标 | 值 |
| --- | --- |
| 冷启动首请求 | **122 ms**（控制器就绪 118.5 + 首请求 3.5） |
| 延迟 P50 / P95（回环，warm 轮） | 2 ms / 25 ms（30/30 成功） |
| 公网 204 P50（经内核 DIRECT） | 135 ms（10/10） |
| 回环吞吐 | 1116–1384 MB/s |
| 内核 CPU / 内存峰值 | 0.4%（峰 0.8%）/ ~30 MB |
| 系统解析器 DNS P50 / P95 | 1 ms / 26 ms |
| 重启恢复（杀→重启→首个 204） | **109 ms** |

### 待补矩阵

| 维度 | 状态 |
| --- | --- |
| TUN 模式（gvisor/native DNS/fake-ip） | 夹具与命令已备，**待受控设备/虚拟机**（需 sudo 建 utun 改路由） |
| sing-box | 转换夹具待补（DNS 形状见下审计），装核后复跑 |
| Windows | 待补 |
| 弱网/丢包/受限 UDP/IPv6 异常/门户 | harness 可在对应网络环境复跑并记录实际出口；待实机 |
| 其他 VPN 共存 | 见 #490 的归属跳过逻辑；基线层面记录实际出口即可 |

## DNS / 协议 / TUN 路径静态审计（#492 第 2 条）

### mihomo 运行时注入（runtime.rs）

- **TUN**：gvisor + auto-route + auto-redirect + auto-detect-interface；
  双栈仅在物理网络有原生 v6 时注入（#136），v6 侧排除链路本地/ULA/组播；
  私网全段 route-exclude（#145，含 CGNAT/Tailscale 100.64/10）。
- **DNS**：TUN 开启时 native 模式 + `dns-hijack any:53`（含 TCP 53）；
  fake-ip + `ipv6` 与 inet6 同门控。**fake-ip-filter 未显式配置**——依赖
  mihomo 默认列表（`*.lan`/`*.local` 等）；企业自定义内网命名（如 `.corp`）
  会得到 fake-ip，域名层是否落到私网 DIRECT 取决于订阅规则——**待动态验证**
  （基线夹具为 DIRECT-only，不暴露此差异）。
- **嗅探**：TLS 443/8443 + HTTP 80/8080-8880，override-destination（#331
  污染 IP 无害化）。
- **私网 DIRECT 规则**前置注入（no-resolve），先于订阅兜底规则。

### sing-box 转换（singbox_convert.rs）

- `proxy-dns`（DoH 8.8.8.8，detour = 最终出口）与 `local-dns`（223.5.5.5，
  直连）分离；`route.default_domain_resolver = local-dns` → **节点域名引导
  解析走直连，无依赖环**。
- DoH server 为 IP 直写，无 DoH 引导解析需求。
- 规则集下载 detour = 最终出口（订阅端语义决定走代理与否）；应用侧 geo
  文件预下载走 gh-proxy 镜像。

### 待动态验证

- 内核自身 DNS 上游查询在 TUN 场景的出口（mihomo 内部排除，静态判断无环；
  需抓包确认）。
- fake-ip 默认过滤列表对自定义内网命名的覆盖（上节）。
- IPv6 异常/UDP 受限环境下的双栈回退行为（#136 门控的边界）。

## 实验纪律

每轮只变一个变量；同一夹具与端点跨轮复用；涉及节点/协议切换时在结果中
注明服务端与线路差异。默认值变更（如 gvisor→system、fake-ip 范围、DNS
上游）必须引用本目录的对照数据并单独提项。
