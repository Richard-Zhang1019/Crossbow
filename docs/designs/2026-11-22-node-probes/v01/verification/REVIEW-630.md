# PR #630 评审补充（2026-11-22）

原始 v01 源稿、截图和公网证据不覆盖；本页及 `review-630*` 是后续评审证据，当前行为以正式源码和 `docs/NODE-PROBES.md` 为准。本次未改 UI。

Issue #604 的分项测试缺口在 base b7a2133 上成立。独立最小内核优于切主策略或只测当前混合端口，可保留；guardian、按需发包、有限负载与分目标历史设计也保留。

发现并修复 P2：ureq 的 500ms `timeout_connect` 包含 SOCKS 远端拨号，并非仅连接回环代理。受控回归中 700ms 拨号在 3s 阶段预算内，却在 499ms 被判失败。移除此额外短上限，使用阶段总体截止时间；修复后 700ms/3s 成功，700ms/100ms 仍按预算失败。HTTPS、下载、上传共用 agent，均覆盖此修复。

## 已执行

- 后端 384 项通过，8 个显式原生用例默认忽略；Clippy all-targets `-D warnings`、fmt 通过。
- TypeScript/Vite 构建、前端 47 项测试、生产组件分项测试脚本及既有质量历史脚本通过；实际检查浅色中文与 880×560 深色英文渲染。
- Mihomo 1.19.32、sing-box 1.14.2：各自验证 TCP 成功/HTTPS 失败、HTTP 204/UDP 黑洞且不超过 3 包、取消、模拟 GUI 父进程 SIGKILL 后仅回收所属内核与私有目录，共 6 项通过。使用最终生产 Worker/guardian，全部为 loopback 夹具。
- 修复前 CI 已通过；推送修复后不沿用旧绿灯，新 CI 状态待定。

## 复跑

在完整仓库根目录，桌面 Rust lib 测试与 desktop binary 已构建：

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib socks_target_dial_uses_stage_budget
python3 docs/designs/2026-11-22-node-probes/v01/verification/review-630-native.py \
  --test-bin /path/to/crossbow_desktop_lib-HASH \
  --helper /path/to/crossbow-desktop \
  --mihomo /path/to/mihomo --singbox /path/to/sing-box --output /tmp/probe-review.json
```

未重跑公网吞吐/干扰采样，原始 native.json 属于作者提交时的证据。原生窗口完整 Tauri IPC/安装、Windows Job 与 Linux 实机仍待验；CI 构建通过不能替代这些验收。
