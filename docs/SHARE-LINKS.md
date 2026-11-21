# 分享链接支持子集（#599 / PR #616）

本地解析器经 `subscription::normalize_profile_content` 生成 mihomo 配置，不使用在线转换服务。列表最多 8 MiB / 4096 节点；任一行失败整批失败。完整 Clash / sing-box 配置继续按既有规则保留，不套用 URI 列表能力表。

## 本次新增

| 输入 | mihomo v1.19.32 | 当前 sing-box 转换路径 |
| --- | --- | --- |
| hy2 / hysteria2，单段密码，默认 443 / 显式端口，SNI | 支持；默认验证证书 | 基础字段可转换 |
| Hy2 salamander + 密码 | 支持 | 明确拒绝，尚未映射混淆语义 |
| TUIC UUID + 密码 + 显式端口，SNI、ALPN、bbr/cubic、native/quic | 支持所列子集 | 明确拒绝该协议 |
| VLESS TLS / REALITY 与 TCP vision | 支持所列子集 | flow、REALITY、client-fingerprint 不能等价转换，明确拒绝 |

这不是内核本身功能完整性的比较，只描述 Crossbow 当前的 URI → mihomo → sing-box 转换能力。导入/引擎切换调用既有渲染预检时，可拒绝不支持的转换；本次未新增 UI 能力面板或改变确认流程。

## 严格边界

- 密码和名称按 UTF-8 百分号转义解码；查询参数保持 form 编码的 `+`/`%2B` 区别。不接受残缺/非法 `%` 或会导致替换字符的无效 UTF-8。
- 协议查询参数采用白名单；重复项、冲突别名、未知扩展和不支持的传输拒绝，不静默删除。
- REALITY 公钥须为 32 字节 URL-safe Base64，输出规范化为无填充编码。short-id 为最多 16 个十六进制字符、偶数长度；vision 需 TLS/REALITY 且仅 TCP。
- 未设置时不自动关闭证书校验；显式提供的合法 insecure 参数按输入保留。字段错误不回显原 URI、认证密码或不支持的参数值。
- Hy2 userpass 双段认证、无认证、端口跳跃、pinSHA256、ECH、gecko 仍未支持；TUIC 的 zero_rtt 等扩展仍拒绝。这是首个受控子集，不表示这些形式在协议规范中非法。
- Hysteria 官方规范实际定义了 userpass 双段认证；当前拒绝只是实现范围，不应称为规范中的歧义。

## 验证与证据

核心单元 171 项、DNS 集成 6 项、桌面后端 322 项通过；core Clippy all-targets、格式检查通过。新回归覆盖错误转义、密码字符保持、公钥规范化、跨内核不丢参，以及基础 Hy2 的 TLS/SNI/密码映射。

可选原生检查经公开导入入口生成三组脱敏配置（Hy2 IPv6/转义/混淆、TUIC 认证/ALPN/UDP、REALITY 填充公钥/vision），通过 mihomo v1.19.32 `-t`，不访问真实服务端、不改变系统代理：

```sh
CB_MIHOMO_BINARY=/path/to/mihomo cargo test -p crossbow-core --test share_links_native -- --ignored --nocapture
```

原始填充公钥直接交给该内核会报 `invalid REALITY public key`；规范化后通过。

仍待：多种真实客户端导出结构的脱敏夹具、三协议受控服务端连接和 TLS/SNI/认证/UDP 端到端验证、按内核/版本展示的 UI 能力表。配置 check 和单测均不能代替这些验收，#599 不应因首个子集而被认定全部完成。

公开格式与字段来源：[Hysteria URI](https://v2.hysteria.network/docs/developers/URI-Scheme/)、[mihomo TUIC](https://wiki.metacubex.one/config/proxies/tuic/)、[mihomo VLESS](https://wiki.metacubex.one/config/proxies/vless/)。夹具为依据公开格式构造的合成数据，不冒充真实客户端导出样本。
