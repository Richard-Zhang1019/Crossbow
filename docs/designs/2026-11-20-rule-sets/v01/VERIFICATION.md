# 最终验证（2026-11-21）

最终实现 `ec3fff5`，同步 main `f2d91ed` 后的代码为 `f210ec9`。设计原始基线 `540101e` 保留于 SPEC；同步 main 未改变规则集页面设计。

- core：110 通过、2 忽略；桌面后端：317 通过、2 忽略。
- 单独启用规则集真实双内核用例通过。默认忽略的其他原生用例不算作本次通过证据。
- workspace Clippy `--all-targets -- -D warnings`、rustfmt、前端 TypeScript/Vite 构建及 43 项前端测试通过。Vite 既有 chunk 大小警告仍存在。
- 最终生产组件 + IPC 夹具通过加载、启停、排序、失败草稿/旧数据保留、成功重试、新增、空状态、深浅/中英文和小窗口可见性检查。
- 真实 mihomo / sing-box：缓存 provider 配置校验通过；本地 HTTP 请求的启用/禁用 chains 见 [结构化结果](previews/native-routing.json)。进程与端口都在验证结束后清理。

未启动安装版原生 UI，未改变用户系统代理/TUN 或订阅。Windows 原生流程、实际社区大规则源长期更新、用户网络切换/托盘/轻量退出体验仍待验。轻量退出会停止应用调度器，独立内核继续消费已有缓存，重开应用后恢复更新。

可复跑入口见 SPEC 和 `scripts/dev/verify-rule-sets.mjs`。PR/CI/发布状态以 GitHub 为准。
