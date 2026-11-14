# 首次连接引导 v02

已实现，待评审。关联 #484、#467 与 PR #483。

沿用 Crossbow 的 Frame Studio 外框、主题色与字体。新版把首次操作放在已有状态旁边，连接路径和流量趋势继续构成左侧主区。

- 无配置时，标题下显示「导入配置」和一行说明。
- 「内核进程」行提供下载、启动或重试。下载进度和端口占用的处理提示放在状态卡底部。
- 「接管配置」行的「前往开关」将焦点移到现有开关，用户可选择系统代理或 TUN。
- 「网络连通性」行提供「测试 GitHub」。测试中显示按钮进度，通过后原位显示「GitHub 可达」，未通过时同一行提供重试。测试走 Crossbow 混合端口，测试期间接管开关可继续使用。

## 运行与预览

在仓库根目录运行以下命令，再打开 [本地预览](http://127.0.0.1:5246/)。

```sh
pnpm --filter @crossbow/desktop exec vite --config ../../docs/designs/2026-11-13-first-connection/v02/vite.config.mts
```

默认进入验证阶段。查询参数 `stage` 支持 `import`、`download`、`start`、`takeover`、`verify` 和 `established`，可附加 `dark`、`en`、`probeFail`、`downloadFail` 或 `busyPort` 查看相应状态。

[浅色验证](previews/verify-light.png) · [验证通过](previews/verified-light.png) · [最小窗口深色验证](previews/verify-dark.png) · [最小窗口失败重试](previews/retry-dark.png) · [初次导入](previews/import-light.png)

源稿复用桌面生产组件与 v01 的脱敏 IPC 模拟。浏览器覆盖 1120 × 730 中文浅色、880 × 560 英文深色和 1440 × 900 常规配置。真实代理、下载与系统授权沿用原生流程，本次验收在浏览器模拟环境完成。
