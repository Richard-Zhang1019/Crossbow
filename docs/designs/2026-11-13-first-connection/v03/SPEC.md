# 首次连接引导 v03

已实现，待评审。关联 #484 与 PR #485。

正式实现见 [概览页面](../../../../apps/desktop/src/pages/Home/index.tsx)和[概览样式](../../../../apps/desktop/src/styles/layout.css)。

「连接路径」标题行右侧显示「测试 GitHub」，用主题强调色、活动图标和文字表示可点击。文字入口有 24px 点击高度，与标题垂直对齐，鼠标悬停时文字出现下划线，键盘操作时显示焦点轮廓。与下方节点同名时省略标题行的重复名称。窗口变窄时优先截断名称。

「连接状态」里的网络连通性与另外两行共用布局。待测试和正在测试使用次要文字色，通过时显示绿色对勾与「GitHub 可达」，未通过时显示警示色与「未通过」，并在路径标题原位提供重试。点击测试后只锁定该动作。成功后收起入口，若焦点仍在入口则移至结果。结果只说明 GitHub 经由本地代理端口的本次测试。

## 运行与预览

在仓库根目录运行以下命令，再打开 [本地预览](http://127.0.0.1:5248/)。

```sh
pnpm --filter @crossbow/desktop exec vite --config ../../docs/designs/2026-11-13-first-connection/v03/vite.config.mts
```

默认进入验证阶段。可附加 `?dark&en&probeFail` 查看深色英文与失败重试。其余场景沿用 [v02 预览参数](../v02/SPEC.md)。预览使用桌面生产组件和脱敏 IPC 模拟。

[浅色待测试](previews/verify-light.png) · [浅色已通过](previews/verified-light.png) · [最小窗口深色待测试](previews/verify-dark.png) · [最小窗口深色重试](previews/retry-dark.png)

浏览器已验证待测试、测试中、通过、失败重试、键盘操作和焦点移交。真实代理连通性与原生窗口留待实机验收。
