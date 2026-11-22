# 便携配置包 v2：个人策略迁移

2026-11-22 · #607 · `codex/607-portable-policies` · 已实现，本地/生产组件模拟/隔离原生通过，待 PR 评审与实际换机验收。

## 现状与最佳方案

旧 ConfigPayload 只有 profiles/overrides/active_profile。最小改动可直接增加无版本约束的字段，但旧工具会忽略字段，无法区分迁移范围与冲突。最佳方案升级便携包 v2，读取 v1/v2，保留身份与顺序，确认前展示脱敏差异，并沿用现有配置事务。它不复制整个本机 Store，也不引入云同步或加密承诺。

进程规则与独立规则集（元数据 + 已验证缓存）整体携带；原始配置、覆写绑定和当前选择沿用现有逻辑。SSID/DNS/内核/权限/接管状态排除。**域名/IP 用户规则（#605，v2 定稿后才进入 main）不在本版包承诺内**：导出不携带，导入不覆盖本机条目；后续可扩展版本化字段。

## 交互与视觉

沿用现有配置包 Modal、Button 和 Notification。预览按配置/覆写、进程规则、规则集显示新增/替换数量；同名不同 ID 明确保留，进程名/规则集重名冲突时禁用确认。原始 Profile 无法验证的目标给出计数说明，确认后按有效覆写结果校验。长范围说明放在可键盘展开的“迁移范围与安全说明”，避免默认预览成为整屏文字。

- [改前](previews/before.png)：main 基线配置包生产组件，1120×734，浅色中文。
- [改后](previews/preview.png)：新增个人策略数量与提醒，同尺寸同主题。
- [导出](previews/export.png)：未加密与排除范围说明。
- [冲突](previews/conflict.png)：冲突说明和禁用确认。
- [小窗口](previews/dark-small.png)：880×560，深色英文。

截图是正式 Profiles 组件 + 合成 IPC，不是原生文件选择器。订阅名称/页脚版本是夹具值。改前在独立 worktree 临时载入 main 的 Profiles/i18n 源文件生成，随后自动恢复。源稿快照及哈希见 source.zip/manifest。

## 验证及边界

- Core：181 项单元及现有集成通过；新增 v1 保留目标策略、v2 清洁库往返/重开/重复替换、冲突无修改、无效缓存/格式、失效目标拒绝和事务写失败原内存/文件不变。最后 ID/周期/同名计数修改又做配置包专项和 Clippy。
- 桌面后端全量 358 项通过；最终快照策略恢复与指纹修改后 workflow 6 项及桌面 Clippy 再通过。
- 前端类型/Vite、47 项测试、既有 10 项生产组件场景，以及配置包专项取消/冲突/保存失败/最小窗口通过。
- 实际启用的原生用例：显式 CB_PACKAGE_MIHOMO=mihomo v1.19.32，经导出→导入新临时库→落盘→重开→render_config；确认缓存 IP 规则阻断回环 HTTP，停用后返回已知 direct 响应。进程规则恢复内容和内核接受已核对；未证明所有进程的实际归属。无系统代理/TUN/真实订阅操作。
- 待验：两台真实机器、原生文件选择器、运行中用户配置重载/失败恢复、sing-box 及其他宿主。原生本地测试不等于完整换机验收。

复现（仓库根目录、Node 24、有前端依赖）：

```sh
node scripts/dev/verify-package-policies.mjs
CB_PACKAGE_MIHOMO=/path/to/mihomo cargo test -p crossbow-core native_migrated_cache_blocks_and_disabling_restores_direct -- --ignored
```

详细协议与合并边界、v1/v2 示例见 [配置包说明](../../../config-package/README.md)。新 UI 脚本是本地可复现场景，未声称已加入 CI 固定列表；Core/后端常规 CI 会执行新单元测试，原生用例默认忽略。
