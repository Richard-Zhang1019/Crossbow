# 用户域名/IP 规则 v01

2026-11-22 · #605 · `codex/605-user-rules` · 已实现，本地/模拟/受控原生通过，待评审与实机验收。

## 方案

最小方案是生成覆写规则；它耦合列表替换、排序与安全模式。最佳方案为结构化 `user_rules`，复用既有配置事务、统一通知和 ProcessRules 的表格/编辑器样式。无需建立另一套匹配引擎。顺序与字段边界见 [USER-RULES.md](../../../USER-RULES.md)。

## 设计

沿用主题、按钮、选择器、输入、弹窗及连接/进程表格的分隔线。列表展示类型、序号、值、目标、状态和排序入口；删除放在编辑弹窗。优先级说明始终显示，最小窗口也不隐藏；长条件可折行。只有规则确认后才保存，不在连接页自动写入宽规则。静态覆盖与内核命中记录分开标识。

## 生产截图与源稿

- [改前](previews/before.png)：main 基线配置来源页（没有用户规则入口），1120×734，浅色中文。
- [改后](previews/light.png)：生产组件 + 合成 IPC，1120×734，浅色中文。
- [小窗口](previews/dark-small.png)：880×560，深色英文。
- [编辑器](previews/editor.png)：同上，IPv6 编辑与删除/确认入口。
- [保存失败](previews/failed.png)：模拟写入失败，保留编辑输入。

模拟订阅名称、版本和页脚是夹具值，不表示该版本原生安装已验收。改前在隔离 worktree 暂时载入基线 App/Profiles 源文件，生成后恢复。source.zip 为正式源稿快照，manifest 记录基线与哈希。

## 验证

- 后端 349 项通过，核心 178 项单元及集成通过；新增规则校验、覆盖边界、过期保存及失效策略拒绝回归。
- TypeScript/Vite、前端测试、既有 10 项生产组件场景、用户规则专项交互与 Clippy 通过。
- [原生证据](verification/native.json)：macOS arm64，mihomo 1.19.32 / sing-box 1.14.2，22 个隔离真实路由用例；不启用系统代理/TUN。本版未验证真实用户配置的热重载、订阅更新、权限交接及其他平台。

复现（仓库根目录，有前端依赖、Rust、Python/curl 和指定内核）：

```sh
node scripts/dev/verify-user-rules.mjs
python3 docs/designs/2026-11-22-user-rules/v01/verify-native.py --mihomo /path/to/mihomo --singbox /path/to/sing-box --evidence /tmp/user-rules-native.json
```
