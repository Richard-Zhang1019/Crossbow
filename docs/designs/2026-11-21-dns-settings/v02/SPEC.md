# DNS 设置 v02：受控能力与原子应用

状态：已实现并完成本地/生产组件模拟检查；待 PR #617 合并及原生网络验收。关联 #598。v01 留作历史，不代表当前标准。

## 操作与层级

沿用 Crossbow 的 Frame Studio 设置行、共享 Input / SegmentedControl / Button、冷灰主题和字号 token。不新增嵌套面板。设计知识库用于约束内参考：科技极简的灰阶层级，网格对齐仅作辅助，不改变产品配色或字体。

- 模式与三个解析用途保持设置行。默认窗口输入宽 340px；窄窗口输入位于说明下方并占满内容列，不沿用短端口输入的 130px 限宽。
- 输入提供真实 label；占位符仅示例。模式选择、加载/禁用和键盘焦点由共享组件提供。
- 删除重复用途说明列表；保留会影响决策的覆盖优先级、专用策略及 IPv6 边界。小窗口仍可滚动阅读。
- 保存按钮位于本组右下方，按内容宽度显示。无变更时禁用；应用期间冻结草稿。
- 初次读取失败时禁用编辑并提供重试。保存失败保留输入供修正，确认设置保持旧值。通知统一归属 `dns-settings`；取消二次 `apply_overrides` 调用。
- 能力说明随设置页确认的引擎变化。sing-box 的独立节点引导项禁用并解释原因。

## 能力及优先级

| 用途 | mihomo | sing-box 的 Clash 转换路径 |
| --- | --- | --- |
| 默认/代理业务查询 | `nameserver` 显式指定 MATCH 策略；无代理上游时使用直连上游并指定 DIRECT | `proxy-dns` 经 MATCH；DIRECT 策略省略 detour |
| 直连业务 | `direct-nameserver` 显式 DIRECT | `local-dns` 直连拨号 |
| 节点引导 | 显式 DIRECT；未填时用直连上游；仅代理上游且无引导路径时拒绝保存 | 不支持独立设置，使用 local-dns；非空独立引导项明确拒绝 |
| 多上游 | 每类最多 8 条 | 每类恰好一条；额外条目拒绝，避免静默只使用第一条 |
| DNS 服务域名引导 | 仍遵循 mihomo 默认引导配置 | 直连上游必须 IP；代理 DNS 服务域名经 local-dns 解析，避免循环 |
| 原生 sing-box JSON | 不适用 | 沿用原配置；自定义模式明确拒绝，不假报已应用 |

自定义覆盖覆写/订阅同名上游字段；mihomo 的 hosts、专用 nameserver-policy、fallback 保留，专用策略可能使用其他上游。**不是全局 DNS 防泄漏承诺。** TUN 最后注入运行时 enable/mode/IPv6 等，保留设置的上游。Clash DNS/hosts 的 sing-box 转换仍遵守 #593 守卫，不能声称“自动使用默认 DNS”。

支持受控 HTTPS/TLS/UDP/裸地址、方括号 IPv6、非默认端口；拒绝凭据、查询参数、片段、非法端口及会被引擎忽略的路径。不添加跳过 TLS 验证选项。

## 事务

`set_dns_settings` 内校验能力与候选渲染，通过现有配置事务应用、落盘并在失败时恢复旧快照，不再先落盘再应用。停止的内核也预检渲染；无活动配置时允许合法初始化设置。引擎切换也检查 DNS 能力。运行中的 mihomo DNS 变更参与既有出口前后探测和失败恢复；探测未确定不冒充整个 DNS 路径验证。

## 可复现证据

`source.zip` 保存可编辑生产源文件及验收脚本；仓库根目录安装锁定依赖后运行：

```sh
CB_DNS_EVIDENCE=/tmp/crossbow-dns-evidence node scripts/dev/verify-dns-settings.mjs
```

- 改前：原 PR `3133411`，相同合成上游，真实组件渲染；`before-light.png` / `before-dark-small.png`。
- 改后：`light-custom.png`（1120×734，浅色中文）；`dark-small.png` / `dark-small-footer.png`（880×560，深色英文，主体与滚动后页尾）；`dark-singbox-disabled.png`；`load-failed.png` / `apply-failed.png`。
- 自动交互覆盖初读失败与重试、保存失败保留草稿/旧确认值、等待期禁用、单 IPC 提交、成功后禁用重复提交、真实设置页控件切换引擎、切回沿用模式。脚本已登记两处 CI 场景清单。
- 本机 mihomo v1.19.32、sing-box v1.14.2 接受生产渲染的 IPv4/IPv6/非默认端口配置；这是隔离配置检查，不是实际 DNS 解析验收。

仍待：三种用途的实际查询路径、内网/hosts、断网/上游/TLS 失败下的原生恢复、IPv4-only/双栈平台矩阵及安装版体验。不得以构建、配置 check 或模拟截图代替这些结论；#503 的未验组合保持待验。
