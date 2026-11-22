# 自定义策略组 v01

状态：已实现；本地检查、生产组件模拟、macOS 双内核隔离路由检查通过；桌面整机生命周期/其他平台待验。关联 #606。

## 设计决定

- 在节点页侧栏放置次要「自定义组」入口；不增加主工作区的横幅或压缩节点卡片区。
- 统一 Dialog 内完成列表和编辑（不叠弹窗），来源/能力说明与操作在同一处。列表使用已有分隔线和主题变量，编辑复用输入、单选/多选选择器及按钮。
- 订阅组保持只读；来源名、成员规则与最终成员分开表达；显式告知新组不会自动改变流量出口，自动测试保存后会访问测试 URL。
- 明暗主题和英文共用生产组件；小窗口允许 Dialog 内滚动，保留完整参数和底部动作。异步保存失败保留草稿，使用统一通知；预览校验错误在字段区解释，来源竞态禁用保存。

能力模型、实现选择和限制见 [CUSTOM-GROUPS.md](../../../CUSTOM-GROUPS.md)。

## 可复现验证

在仓库根目录、已安装桌面依赖及本机 Chrome 的环境：

```sh
node scripts/dev/verify-custom-groups.mjs
# 截图默认 /tmp/cb606-ui，也可设置 CB_GROUP_EVIDENCE
node scripts/dev/verify-scenarios.mjs --scenario 4
```

双内核隔离验证（需本机安装的 core，不下载，不访问真实订阅）：

```sh
python3 docs/designs/2026-11-22-custom-groups/v01/verify-native.py \
  --mihomo /path/to/mihomo --singbox /path/to/sing-box --evidence /tmp/custom-groups-native.json
```

脚本通过忽略的显式 fixture exporter 写入 Store、重新打开并调用生产 renderer；启动两个 loopback 假上游和实际内核，检查控制器候选 A/B 与代理响应。手动组切换 A→B 并确认响应改变，自动测试组确认候选及实际出口属于 A/B。测试自动测试目标也仅在 loopback。

## 本次证据

- 基线：`0af239a`，变更为本 PR；manifest 中源稿 SHA256 固定实际内容。改前从该提交的隔离源码副本渲染；改后直接渲染此 worktree 的生产组件。
- 浏览器：macOS arm64 / 系统 Chromium 157；1120×734 浅色中文与 880×560 深色英文，模拟 IPC、禁止外部浏览器请求。截图页脚的 `0.14.4-verify` 是统一 harness 的固定标识，不是发布版本。
- 模拟覆盖：名称过滤预览、保存失败保留输入、成功后列表刷新、来源变化禁用保存、小窗口/语言/主题/自动测试表单。节点页通用布局场景 4 通过。
- Rust 覆盖：两种组渲染、转换、过滤随节点变化、缺失成员/嵌套/重名/空结果拒绝、来源上下文及组列表 CAS、引用保护、失败事务不写盘。
- 受控原生：Mihomo 1.19.32 与 sing-box 1.14.2，共 4 个成员/路由/保存重开场景，结果见 `verification/native.json`。未动用户系统代理、TUN 或真实订阅。
- 待验：真实桌面端添加/编辑/删除完整流程；真实订阅更新失败/回滚、系统代理或 TUN 接管期间变更；Windows/Linux 原生和 CI。PR 提交不表示以上全部验收。

`source.zip` 为可编辑生产源稿快照，实际依赖以仓库锁文件为准。历史稿不覆盖；正式标准以当前生产组件及 CUSTOM-GROUPS.md 为准。
