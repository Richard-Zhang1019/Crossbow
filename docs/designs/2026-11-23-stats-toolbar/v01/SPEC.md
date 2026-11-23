# 流量统计工具栏统一规格

状态：用户已确认，已实现，生产组件模拟验证通过；原生 WebView / CI 待验。
关联：Issue #644，PR #645。日期：2026-11-23。

## 用户目标与根因

时间范围、刷新、暂停/继续记录、清除本地统计属于同一工具栏，应有一致的高度、字级及垂直对齐。旧实现范围切换34px、刷新32px、两个文字按钮28px，字号12/11px混用，因此右侧文字按钮显得偏小。

## 正式规格

全部复用已有组件：SegmentedControl 不变，三个 Button 使用 `context="toolbar"`，不使用 `size="small"`。

- 高度由 `--cb-control-height-toolbar` 提供（34px），字体由 `--cb-type-control` 提供（12px）。
- 刷新为34px正方形，文字按钮宽度随文案自然展开；不要求不同长度按钮等宽。
- 沿用公共 Button 的边框/圆角、hover、键盘焦点、危险色、加载与禁用反馈；保持统计页已有换行布局。
- 不改变统计数据采样、保存、暂停、清除或事件顺序；不改全局默认按钮尺寸。

## 验证与复现

基线 `825c295`；实现 `553342b21cba71a055f828002fb4246f30ce5e4e`。截图为生产 React 页面、合成 IPC，页脚测试版本标识不代表发布版本。无真实订阅/网络凭据。

| 场景 | 调整前 | 调整后 |
| --- | --- | --- |
| 1120×720 浅色中文 | [截图](previews/before-light.png) | [截图](previews/after-light.png) |
| 1120×720 深色中文 | [截图](previews/before-dark.png) | [截图](previews/after-dark.png) |
| 880×560 深色英文 | [截图](previews/before-small-en.png) | [截图](previews/after-small-en.png) |

`before-checks.json` / `after-checks.json` 为浏览器实际高度/字体测量；改后三个场景均为34px/12px，工具栏无横向溢出。

从仓库根目录安装现有依赖后执行：

```sh
STATS_EVIDENCE=/tmp/cb-stats-toolbar node docs/designs/2026-11-23-stats-toolbar/v01/preview.mjs
STATS_EVIDENCE=/tmp/cb-stats-regression node scripts/dev/verify-stats.mjs
node scripts/dev/verify-design-tokens.mjs
```

归档脚本渲染当前工作区生产组件；对比前态需在基线工作区运行该脚本，不能把当前截图标作旧版。可编辑生产源快照及预览脚本见 `source.zip`。

已有统计回归通过范围切换/迟到请求、暂停与继续、控制读取/写入失败、清除后刷新、空态和明暗/小窗口。此次仅补充文档及既有证据，未重复无变化的行为测试，也未运行完整桌面打包。原生 Tauri/WebView 与本次提交 CI 待验，未持续轮询。
