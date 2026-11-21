# 轻量退出后的内核守护（#601 / #622）

## 问题与原复现证据

main 基线存在真实缺口：轻量退出完全结束 GUI，原来位于 GUI 内的崩溃回调不再运行；保留的内核随后死亡，会留下指向失效端口的系统代理。普通关窗保留托盘不属于这个场景。

原作者在 macOS v0.15.2 中记录：GUI/内核与代理正常，代理请求 HTTP 204；SIGKILL GUI 后内核保持运行，再 SIGKILL 内核，代理请求 connection refused，至少 60 秒未恢复；重开 GUI 后恢复。该实验说明 GUI 不在时的缺口，**不能代替实际轻量菜单退出、LaunchAgent 注册和收养交接验收**。

## 当前方案与边界

macOS 用户级 LaunchAgent 使用现有二进制的 `--kernel-guard <绝对数据目录> <会话 ID>` 模式，不初始化 Tauri/WebView，不启动或杀死任何内核。

- 轻量退出沿用 `prepare_handoff`：持久化描述符、保留内核与代理。仅有合法非零端口、非空代理快照且内核 PID/二进制身份可核对时才注册守护。
- 私有 `kernel-guard-session.json` 绑定随机 ID、原 journal 和描述符的 SHA-256，不重复保存控制器凭据。任一文件变化或会话撤销后，旧守护退出。
- 每 10 秒核对 PID/二进制身份、混合端口与带认证的本机控制器 `/version`。探针禁止使用环境代理，不访问外网。身份未知时保持观察；连续两轮明确异常才恢复，减少睡眠/切换时短暂异常的影响。
- 不再按固定 24 小时丢弃仍有效的轻量会话。内核存活多日时继续保护；GUI 接管、契约失效或完成一次恢复后退出。资源开销需要实测，不能把目标数值当成结果。
- 恢复前使用 `kernel-guard-recovery.lock` 和现有 `app.lock`，在锁内重新核对契约与健康。GUI 启动若遇到守护正在恢复，最多等待 60 秒；取得实例锁后才撤销旧会话并注销 LaunchAgent。不会在恢复中途 bootout 写入进程。
- 复用 SysProxyManager 逐服务归属恢复，写入限定为无人值守入口。第三方端点、主动关闭的代理、PAC/WPAD 均跳过；旧快照中的失效本机代理不会重新写回。恢复失败保留 journal。
- 结果明确区分“实际恢复了归属服务”“无归属服务可恢复”“恢复失败”。跳过不提示恢复成功；恢复成功只说明按快照恢复设置，不承诺已直连或网络可用。
- 脱离 GUI 的结果经统一通知服务投递系统通知，并私有保存一条脱敏 Notice，下一次 GUI 启动加入原通知队列。注册失败也明确报告；不阻止原来的轻量退出。
- 注册检查 launchctl 的实际退出状态，有 5 秒超时；plist 路径进行 XML 转义。启动日志私有并在注册时截断，不持续记录每轮心跳。

不自动重启内核，不重新开启用户关闭的代理，不继续执行 GUI 里的订阅调度/统计等任务。

纯 TUN 且没有系统代理恢复契约时不启动该守护；本实现只恢复系统代理设置，不处理 TUN 设备、路由或 DNS，不保证 TUN 崩溃后物理网络会自动恢复。Windows/Linux 不启用此 LaunchAgent 模块，各自服务体系行为需要独立验收。

## 验证入口与待验

常规回归：`cargo test --locked -p crossbow-desktop --lib lifecycle::kernel_guard`。
覆盖契约变化/取消、探活迟到、GUI 实例锁、恢复结果发布期间互斥、第三方/PAC/主动关闭、写入失败保留 journal、PID 复用、非法 journal 与 launchctl 非零/超时。

原生隔离探活：设置 `CROSSBOW_CORE_BIN` 为 mihomo 可执行文件，运行 `cargo test --locked -p crossbow-desktop --lib native_core_hang_is_unhealthy_even_with_a_listener -- --ignored`。该测试只操作自己拉起的内核和随机 loopback 端口：正常、SIGSTOP（端口仍监听）、SIGCONT、结束进程。不写系统代理。

仍需在本轮修复提交上验收：

1. 实际轻量菜单退出、LaunchAgent 注册/运行、重开收养、普通退出和更新，记录 GUI/内核 PID 与连接连续性。
2. 真实系统代理恢复、第三方代理共存、授权失败和系统通知送达。单测替身不等于系统代理实测。
3. 睡眠唤醒、切网、恢复中的 GUI 重开，以及守护常驻 RSS。
4. TUN 路由/DNS 生命周期与其他平台服务能力，不由本轮守护推断通过。
