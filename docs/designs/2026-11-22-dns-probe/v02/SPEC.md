# DNS 诊断评审修订 v02

2026-11-22 · #603 / PR #624 · 基线 1a39c8b · 已实现，本地/模拟/隔离原生通过，CI 待推送后执行。

## 保留与修正

保留已有折叠卡片、系统/内核并列结果、TTL/未知字段、最多五条本次会话历史及白名单复制。复用内核接口的方案比自建 DNS 客户端更合适，但不能把控制器查询等同于每条业务连接。

- 系统应答按 A/AAAA 分族过滤、去重并限制四条。无目标地址族时返回无记录，避免 A 成功掩盖 AAAA 失败。
- 请求绑定 node_delay_revision、profile_id、PID 和运行状态；配置事务开始时该代次递增，结束前比较，变化即拒绝结果。界面另以本地请求序号/诊断 revision 丢弃迟到结果，卸载、取消、清除后不复活。
- 单条历史即可清除。取消只取消展示，不承诺终止不可取消的 OS getaddrinfo；后端只允许一个 DNS 对比任务及一个阻塞系统解析工作，旧 OS 查询超时未退出时新系统查询返回不可用。
- 控制器单请求预算为五秒，系统等待预算五秒，version/系统/查询顺序执行通常最多约十五秒；DNS JSON 响应上限 256 KiB。只把可证实的控制器往返超时归为 timeout，不猜上游 TLS 根因。
- 界面显示来源/代次，明确 DNS 请求会发给已配置解析服务，无落盘/遥测不等于不联网。

## 能力边界

[mihomo v1.19.32 官方处理器](https://github.com/MetaCubeX/mihomo/blob/v1.19.32/hub/route/dns.go)调用 DefaultResolver.ExchangeContext。该事实不足以证明所有业务连接（含 hosts、FakeIP、分流/代理端解析）走相同路径。v01 的“真实业务解析”措辞已收窄为“内核 DNS 接口查询”。上游、缓存、逐连接路径及上游 TLS 根因不能由本次接口结果推断。

## 证据

- [修订前](previews/before-review.png)：v01 生产组件模拟截图；保留历史，不覆盖。
- [修订后](previews/04-result-system-only.png)：1120×734，浅色中文，显示接口能力边界与配置关联。
- [最小窗口](previews/07-dark-small-en.png)：880×560，深色英文，可滚动查看完整结果。
- [取消/过期丢弃](previews/08-cancelled-stale.png)：旧结果没有重新出现。

本次后端全量 358 项通过，最终额外边界修改后 DNS 专项 9 项及 Clippy/格式复验通过；前端 47 项、TypeScript/Vite、既有生产组件场景及增强 DNS 场景通过。新增后端回归覆盖地址族、同 PID 配置代次变化和超大控制器响应；增强生产场景覆盖单条清除、取消与切换配置。

真实 mihomo 1.19.32 的隔离控制器测试已实际运行：example.com A 成功，保留 .invalid 域名返回 NXDOMAIN。未更改用户代理/TUN；本轮未重新验证 sing-box、上游 TLS 故障、安装版点击和真实业务流量路径。原 CI 通过只对应基线 1a39c8b，不当作修订提交的 CI 结果。

复现：`node scripts/dev/verify-dns-probe.mjs`；原生用既有 `CROSSBOW_CORE_BIN=... cargo test --lib --manifest-path apps/desktop/src-tauri/Cargo.toml real_core_dns_query_reports_answers_and_nxdomain -- --ignored`。源稿快照见 source.zip，哈希见 manifest。
