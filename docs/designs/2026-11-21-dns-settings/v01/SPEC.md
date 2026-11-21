# DNS 设置卡片 · v01 SPEC（#598 工作包 H1）

- 日期：2026-11-21
- 状态：已实现（随 #598 实现 PR 提交，待评审/实机验收）
- 基线提交：a56c4ac（main，#613 语义守卫合并后）
- 关联：#598（依赖 #593 / PR #613，已合并）

## 用户目标

「我流量怎么没了」类问题的可解释入口：区分节点引导、直连业务、
代理业务与内网/hosts 的解析路径；默认沿用订阅零行为变更。

## 设计决定

- 位置：设置 → 网络，独立 DNS 卡片（LanSharing 同构，复用主题组件）
- 双模式 SegmentedControl：沿用订阅（默认） / 自定义
- 自定义三意图输入：代理业务 / 直连业务 / 节点引导
  （sing-box 不支持节点引导 → 输入禁用 + 说明，不静默忽略）
- 中立意图说明五条常驻卡片内
- 保存 = set_dns_settings（校验先行）+ apply_overrides 配置事务；
  失败保留旧配置，卡片回滚显示真实状态

## 引擎能力表

| 意图 | mihomo | sing-box |
| --- | --- | --- |
| 代理业务 | dns.nameserver | proxy-dns（detour 主出站） |
| 直连业务 | dns.direct-nameserver | local-dns（直连拨号） |
| 节点引导 | dns.proxy-server-nameserver | 不支持（禁用+说明） |
| 内网/hosts | 沿用订阅（H1 不开放） | 沿用订阅 |
| IPv6 | runtime 注入（native v6 检测） | 同左 |

## 交互状态

加载（骨架由既有 settings ready 门控）/ 就绪（双模式）/ 保存中
（按钮 loading）/ 失败（Notification + 卡片回滚显示真实状态 + 重试）/
sing-box 禁用态（节点引导输入禁用 + 说明）。

## 验收边界

- 默认沿用订阅不改变当前配置；启停/订阅更新/内核切换后优先级可解释：
  用户自定义 > 覆写 > 订阅 DNS > TUN 运行时注入（深度合并）
- 格式错误/渲染失败保留旧解析和网络；TLS 校验失败不通过关闭证书
  校验掩盖
- 查询路径的实际证明、无网/上游失败回滚、双栈与内核支持矩阵的
  真机结论 → 实机验收（issue 验收清单逐项跟踪）

## manifest

- 日期：2026-11-21
- 版本：v01
- 基线：a56c4ac
- 来源：#598 issue 设计方向 + 既有 runtime/convert 管线逆向
- 文件清单：
  - SPEC.md（本文件）
  - previews/light-custom.png（浅色 · 自定义 · mihomo）
  - previews/dark-singbox-disabled.png（深色 · sing-box 节点引导禁用）
  - previews/dark-small.png（深色 · 小窗口 880×560）
  - previews/load-failed.png（读取失败 + 重试态）
  - source.zip（可编辑静态源稿）
