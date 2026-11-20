# 网络场景设置

状态为已实现，原生验收待完成。关联 #566 和 PR #580。

## 设计

沿用 Crossbow 的设置页字号、明暗变量和共享控件。参考 design-kb 的工具界面与网格排版方向，用淡底状态行承载当前 Wi-Fi 名称和生效结果。规则表以 Wi-Fi 名称对齐左侧，动作和删除分别放在固定的右列。低风险动作保持常规按钮，删除用已有的危险色图标。窄窗口保留分列，长名称可以省略并通过鼠标提示查看原文。

添加网络时可以编辑名称并显式保存动作。当前网络还没有规则时自动带入名称。已有规则在列表中编辑，选择「跟随全局」会移除规则。短暂读不到 SSID 时仍可管理已知规则，无法确认平台支持或列表读取失败时禁用修改。读取失败有统一通知和重试入口。保存过程中阻止重复提交。

## 预览和源稿

- [改前设置页](previews/before.png)
- [新版局部](previews/detail.png)、[浅色完整窗口](previews/light.png)、[深色](previews/dark.png)、[最小窗口](previews/small.png)、[英文](previews/english.png)
- [添加网络](previews/add.png)、[名称不可用](previews/unavailable.png)、[平台不支持](previews/unsupported.png)、[空列表](previews/empty.png)、[读取失败](previews/error.png)
- [可编辑源稿](source.zip)包含正式组件、样式与回归入口。在仓库依赖安装完毕后运行 `node scripts/dev/verify-scenes.mjs`，可通过 `SCENES_EVIDENCE` 指定截图目录。

通过 Chromium 渲染生产 React 组件录制，测试数据来自脱敏 IPC 夹具。默认窗口 1120×720，最小窗口 880×560。原生验收仍需验证 macOS Wi-Fi 名称读取、定位权限、休眠唤醒、真实代理/TUN 切换与恢复。
