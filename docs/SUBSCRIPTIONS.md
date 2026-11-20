# 本地订阅格式兼容（#569）

导入订阅 URL 时，Crossbow 仍只向用户提供的 URL 拉取内容。Base64 和分享节点链接在本机解析，不调用第三方转换服务。分享内容可从本地文件导入，也可直接粘贴到「添加订阅」的多行输入框（每行一个节点）；Enter 换行，Ctrl/⌘+Enter 导入。分享链接作为本地配置保存，不作为待刷新的远程 URL。导入失败保留输入内容，HTTP 错误也不回显订阅 URL 或凭据。

## 本轮支持范围

- 现有 Clash/mihomo YAML 和 sing-box JSON 保留原文、注释和既有渲染链路。
- 标准及 URL-safe Base64 节点列表，包括省略填充和换行包装。
- `ss://` SIP002（Base64 userinfo / URL 转义凭据）及历史完整 Base64 形式；支持 IPv6 地址。
- `vmess://` v2rayN Base64 JSON：地址、端口、UUID、alterId、cipher、名称、TLS、SNI、证书校验及 TCP/WS/gRPC。
- `trojan://`：密码、TLS、SNI、证书校验及 TCP/WS/gRPC。
- `vless://`：UUID、encryption=none、security=none/tls、SNI、证书校验及 TCP/WS/gRPC。
- WS 的 path/Host、gRPC 的 serviceName。后续选择 sing-box 时，沿用转换器自己的传输能力校验；无法转换的传输会拒绝启动，不伪装成功。

双引擎转换保留显式 TLS 开关；即使服务器使用 443 端口，也不会覆盖 VMess/VLESS 链接明确指定的 `none`。Trojan/Hysteria2 等协议必需的 TLS 保持启用。

链接列表生成一个选择组和 MATCH 兜底规则。重复名称及与内置目标冲突的名称会得到稳定后缀。任意一行解析失败则整份拒绝，提示格式和行号；不会悄悄丢弃坏节点并覆盖当前订阅。错误不回显分享链接、UUID、密码或查询参数值。

## 明确未包含

V2Ray/Xray 原生 JSON 的完整配置转换、SS 插件、REALITY、VLESS flow 和部分扩展传输参数。本轮遇到这些内容会说明限制，不忽略后降级。V2Ray 原生订阅常见的 Base64 URI 列表属于上述支持范围；原生客户端 JSON 不等同于 URI 列表。

可选第三方订阅转换服务未实现，也没有启用入口或默认请求路径。

## 生命周期与验证

远程 HTTP 拉取时归一化，文件/粘贴导入在相同解析器归一化；转换后仍走现有 Profile 的导入、切换、更新、删除及活动配置回滚事务。更新失败保留旧配置。

- 单测：四协议混合列表、Base64/URL-safe/无填充、重复命名、IPv6/转义密码、TLS/WS 字段、两引擎渲染、失败脱敏、配置原文保留及模型生命周期。
- 本机 HTTP 订阅用例：验证远程 Base64 正文归一化并保留流量响应头。
- 可选原生校验：`CROSSBOW_CORE_BIN=/absolute/path/to/mihomo cargo test -p crossbow-core real_mihomo_validates -- --ignored`，只在临时目录执行配置语法校验，不设置系统代理。
- 本地假节点无法证明真实服务器连通性；真实节点测速/出流量、远程订阅实际更新回滚和 Windows/macOS 完整操作流程仍需验收。

协议依据：[Shadowsocks SIP002](https://shadowsocks.org/doc/sip002.html)、[v2rayN VMess 分享格式](https://github.com/2dust/v2rayN/wiki/Description-of-VMess-share-link)、[v2rayN 订阅说明](https://github.com/2dust/v2rayN/wiki/Description-of-subscription)。
