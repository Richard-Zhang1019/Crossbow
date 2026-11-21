# 分享链接支持与导入预检（#599 / PR #616）

本地解析器通过 `subscription::normalize_profile_content` 生成 mihomo 配置，不使用在线转换服务。列表最多 8 MiB / 4096 节点；任一行失败整批失败。完整 Clash / sing-box 配置沿用各自格式，不套用 URI 字段白名单，但仍须通过当前引擎的渲染预检才能导入。原生 sing-box 配置只能由 sing-box 内核读取；当前没有 sing-box → mihomo 转换，预检和引擎切换都会拒绝这一方向，避免 outbounds 被静默忽略。

## 能力范围

| 输入 | mihomo v1.19.32 | Crossbow 的 sing-box v1.14.2 转换路径 |
| --- | --- | --- |
| hy2 / hysteria2，单段或 userpass 认证，默认 443 / 显式端口，SNI | 支持所列子集，默认验证证书 | 基础字段可转换 |
| Hy2 salamander + 密码 | 支持 | 明确拒绝，尚未映射混淆语义 |
| TUIC UUID + 密码 + 显式端口，SNI、ALPN、bbr/cubic、native/quic | 支持所列子集；分开编码及整体转义的认证字段均可解析 | 明确拒绝该协议 |
| VLESS TLS / REALITY 与 TCP vision | 支持所列子集 | flow、REALITY、client-fingerprint 无法等价转换，明确拒绝 |

表格描述的是 Crossbow 的 URI → mihomo → sing-box 转换能力，不是内核自身的全部协议能力。UI 显示当前安装内核版本，并将**已验证基线版本**单独列出；其他版本不据此保证兼容。

## 导入流程与失败保护

1. 粘贴内容后，本地只读检查格式、节点数及当前配置渲染结果；返回状态/协议代码/数量，不返回节点名称、地址、认证内容或原 URI。
2. 检查期间、格式无效或当前内核不能保留语义时禁止确认及快捷键提交。旧请求的迟到结果不覆盖新输入；内核变化后重新检查。检查失败使用统一 Notification，并提供重试。
3. HTTP(S) 订阅 URL 的预览只标记“下载后检查”，不会为了预览发请求。用户确认下载后，与粘贴、文件、深链接共用保存前预检。
4. **保存事务内重新按当前引擎渲染候选配置**，不信任预览时的引擎快照。渲染失败或落盘失败不发布候选，不改变原活动配置、已有订阅或磁盘内容。
5. 已有活动来源时，导入不会将其替换；首次来源沿用既有规则设为当前。导入本身不重启内核或测试真实连接；预检通过也明确提示“尚未测试真实连接”。已有来源更新和激活/引擎切换仍走各自的生命周期校验。

预检针对当前应用 DNS/规则设置和转换路径；它不是每种自定义内核版本的原生 `check`，也不能证明某个远端服务可达。

## 严格边界

- 密码和名称按 UTF-8 百分号转义解码；查询参数保持 form 编码的 `+` / `%2B` 区别。不接受残缺/非法 `%` 或无效 UTF-8。
- 查询参数采用白名单；重复项、冲突别名、未知扩展和不支持的传输拒绝，不静默删除。
- REALITY 公钥须为 32 字节 URL-safe Base64，规范化为无填充编码。short-id 为最多 16 个十六进制字符、偶数长度；vision 需 TLS/REALITY 且仅 TCP。
- Hy2 官方 userpass 按 `user:password` 保留为认证字符串；TUIC 整体转义的 userinfo 仅在已校验 UUID 后按第一个冒号划分，保留密码中的其余冒号。
- 未设置时不自动关闭证书校验；显式提供的合法 insecure 参数按输入保留。错误不回显原 URI、认证密码或未知参数值。
- Hy2 无认证、端口跳跃、pinSHA256、ECH、gecko，以及 TUIC zero_rtt 等仍未支持。拒绝只表示当前实现范围，不表示这些形式在协议规范中非法。

## 验证与复现

### 本地与生产组件模拟

- Core 单元、DNS/订阅集成、桌面后端测试；覆盖转义、混合批次整批拒绝、TLS/密码/公钥保持、不丢字段的跨引擎拒绝。
- 后端事务回归：mihomo 预览通过，保存前切为 sing-box，TUIC 导入失败，旧活动配置和 `store.json` 字节不变；I/O 失败后重试只发布一份来源。
- `node scripts/dev/verify-import-capabilities.mjs`：生产 React 组件 + IPC 桩，覆盖输入与内核变化、迟到结果、禁用确认/快捷键、URL 分类、检查失败重试、最终提交失败保留输入、小窗口深色英文。此项不代替原生 IPC 或真实联网。

### 固定内核的受控本地连接

```sh
python3 scripts/dev/verify-share-links-native.py \
  --mihomo /path/to/mihomo --singbox /path/to/sing-box \
  --evidence /tmp/crossbow-share-links-native.json
```

脚本要求已有 Rust 环境、两个显式指定的内核、openssl 和 curl，不下载内核。公开导入入口生成客户端配置；sing-box 启动隔离 Hy2/TUIC/REALITY 服务端和本地 HTTP/UDP 回显目标：

- mihomo 客户端：Hy2+salamander、TUIC、REALITY/vision、Hy2 userpass、v2rayN 整体转义的 TUIC，分别验证 HTTP/TCP 响应及 SOCKS5 UDP 往返。
- sing-box 客户端：公开转换器生成基础 Hy2 配置，验证 HTTP/TCP 和 UDP 往返。
- 错误认证、错误 SNI、不受信任证书、错误 REALITY short-id 必须拒绝请求。

临时 CA 只注入测试进程，不修改系统信任、不打开系统代理/TUN、不接触真实订阅。mihomo v1.19.32 在构建 outbound 时捕获 CA 池，因此先以无代理配置加载进程内 CA，再经认证回环控制器加载导入配置；没有注入 insecure。临时配置、证书和密钥退出后删除，证据只保留版本、用例名与结果。

2026-11-21，macOS arm64 上 mihomo v1.19.32 / sing-box v1.14.2 的验证结果见本次[设计与验证归档](designs/2026-11-21-import-capabilities/v01/SPEC.md)。这支持已测子集的连接语义，不代表公网服务、TUN、所有平台/版本或所有客户端导出已验收。

### 客户端导出结构来源

[夹具说明](../crates/crossbow-core/tests/fixtures/share-links/README.md)记录固定源码版本、构造方式及范围。v2rayN 和 NekoBox 的 TUIC 导出结构分别覆盖整体 userinfo 转义和独立用户名/密码编码；另覆盖 Hysteria 官方 userpass。夹具全部使用合成值，**由真实客户端的导出源码结构重建，未运行客户端应用抓取实际导出**。后续新增格式必须补来源和字段保持测试，不能把这些样本概括为全客户端兼容。

公开字段来源：[Hysteria URI](https://v2.hysteria.network/docs/developers/URI-Scheme/)、[mihomo TUIC](https://wiki.metacubex.one/config/proxies/tuic/)、[mihomo VLESS](https://wiki.metacubex.one/config/proxies/vless/)。
