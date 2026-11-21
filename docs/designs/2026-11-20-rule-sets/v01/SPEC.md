# 独立规则集管理 v01

状态：已实现，待 PR 评审和用户原生验收。关联 #571。日期 2026-11-20，基线 main `540101e`，macOS arm64。

## 设计选择

用户动作是“添加规则源并选择目标策略”，持续使用时关注顺序、目标、缓存和启用状态。沿用 Frame Studio 现有主题、字号、按钮、选择器、开关、弹窗和监测表格的对齐/分隔线风格，不另造侧栏、彩色卡片或 KPI。设计知识库只用于核对工具型界面的克制和数据层级，项目既有规范优先。

配置增加「规则集」子页；页头解释一次用途。工具行只放条数和添加入口。表格四列：名称/来源与类型、策略目标、缓存规则数/更新时间或失败状态、开关/操作。低频编辑、更新、排序、删除收进共享浮层。空状态解释两步操作；读失败提供重试。长名称按列截断并保留 title。

添加/编辑用共享弹窗：名称、URL、类型/周期双列、策略目标、简短格式说明、取消和保存。保存先下载校验；忙碌时不可重复提交；失败保留草稿。局部覆盖共享标签的独立字段 margin，在 880×560 下完整显示保存按钮，不改变其他弹窗。

后台成功静默；失败复用统一通知。不可见的目标在行内标记“已跳过”，不能把保存状态等同于实际生效。

## 验证证据

生产 React + `scripts/dev/verify-lib.mjs` IPC 脱敏夹具。页脚的 `0.14.4-verify` 是夹具版本，不表示安装版本。规则源均为 example.invalid，名称和 24/48/12 条规则数是明确的演示元数据，不是实际下载的社区数据。

- [改前](previews/before.png)：基线 App/Profiles 入口，无规则集子页；在同一工作区短暂使用 origin/main 组件，截图后自动恢复本次源文件。
- [1120×720 浅色中文](previews/light.png)
- [880×560 深色中文](previews/dark-small.png) · [表单](previews/form-small.png) · [英文](previews/english-small.png) · [空状态](previews/empty.png)
- 浏览器断言：加载、开关、排序、编辑失败不修改旧数据并保留输入、重试成功关闭弹窗、新增、无横向溢出、小窗口保存按钮完整可见。
- 复跑：`node scripts/dev/verify-rule-sets.mjs`，可用 `CB_RULESETS_OUT=/tmp/cb571-preview` 改变输出目录。`--before` 仅用于基线组件。

[source.zip](source.zip) 保留本次生产组件/样式、导航入口和预览脚本的可编辑快照；运行依赖当前仓库，无额外静态复刻稿。

## 原生验证复跑

先指定两个本地二进制并导出生产渲染结果：
```sh
export CROSSBOW_RULE_MIHOMO=/absolute/path/to/mihomo
export CROSSBOW_RULE_SINGBOX=/absolute/path/to/sing-box
export CROSSBOW_RULE_FIXTURES=/tmp/crossbow-rule-fixtures
cargo test -p crossbow-core native_engines_check_cached --lib -- --ignored
python3 docs/designs/2026-11-20-rule-sets/v01/verify-native.py \
  --fixtures "$CROSSBOW_RULE_FIXTURES" --mihomo "$CROSSBOW_RULE_MIHOMO" --singbox "$CROSSBOW_RULE_SINGBOX"
```

真实 mihomo v1.19.32 / sing-box 1.14.2：配置校验通过。回环请求命中时两者 chains 均为 `[DIRECT, Policy]`；禁用后均为 `[DIRECT]`。启用时 controller 规则分别为 RuleSet / rule_set；禁用时分别为 Match / final。验证只改临时配置的监听/DNS测试设施，不改生产渲染的规则及目标；内核关闭后清理临时目录。

这些结果支持规则生成和本地路由结论，不替代实际社区源长期更新、用户系统代理/TUN、托盘与 Windows 验收。完整能力/限制见 [说明](../../../RULE-SETS.md)。
