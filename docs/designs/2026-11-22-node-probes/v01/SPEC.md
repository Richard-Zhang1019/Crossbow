# 节点质量：按需分项测试 v01

日期：2026-11-22（北京时间）。状态：已实现，待 PR 评审和原生窗口验收。关联 #604；第一阶段 #627 已合并。

## 用户操作与设计

入口为节点页的「质量历史」。保留原有延迟历史作为默认视图，新增「分项测试」视图；节点选择和本地记录控制共用，避免将历史的 Google 目标与本次固定 Cloudflare 目标混淆。

分项测试内部以统一 ChoiceChip 切换「连通性 / 有界吞吐」。开始按钮旁展示真实阶段、取消和状态重读；只有显式开始才发包。测试中锁住节点与测试类型，关闭面板取消。结果用纵向键值行呈现 TCP、HTTPS、UDP，未验证不等同永久失败；吞吐独立显示下载与上传确认值。本次与历史结果明确区分，可回看最近 6 次。

沿用 Crossbow 主题变量、Dialog、Button、ChoiceChip、ProgressRing、Select 与统一通知。薄分隔线而非新增多层卡片；说明、操作、结果、记录控制依次排列。浅色/深色、中英文和小窗口保留相同语义层级。

## 最佳方案与边界

最小方案经过当前混合端口测量，无法保证使用指定节点；切换当前策略会干扰用户业务。最佳方案是从已应用配置抽取具体内联节点和拨号链，以独立的最小内核实例进行测试，保留主内核原状态。

任务总预算 30 秒，UDP 至多 3 个数据报，吞吐最多下载 4 MiB / 上传 1 MiB 正文。加密、连接、DNS 和重传开销另计；不是流量账单硬上限。支持常见内联具体节点，策略组、provider-only、网络管理型协议明确拒绝。使用系统 DNS，不等同业务内核自定义 DNS；主机已有 VPN/TUN 仍可能影响路径。

独立 guardian 管道失联或超时后回收其子内核。取消/配置代变化不发布新报告。复用私有哈希历史、暂停/清空、7 天保留；分项报告最多 128 条。详见 [功能规范](../../../NODE-PROBES.md)。

## 实现与可编辑源稿

正式实现：`apps/desktop/src/pages/Proxies/NodeQuality.tsx`、`apps/desktop/src/components/NodeProbes/`、`services/nodeProbes.ts`，后端 `node_probes.rs` 与 `probe_worker/`。`source.zip` 保留该版本生产代码及相关依赖源文件，可在完整仓库依照锁文件运行；非独立打包应用。

前后基线是 `b7a21336c1dce356cabc7d2634114ccbe267819b`，改后为该基线上本 PR 工作树实现。manifest 中记录文件哈希；原生证据另含参与测量的 Rust 源文件和内核二进制哈希。

## 验证证据

- `previews/before.png`：main 基线，1120×734、浅色中文、生产 React + 脱敏 IPC。
- `empty-light.png` / `connectivity-light.png` / `uploading.png` / `transfer-light.png`：1120×734；空态、分层结果、进行中、部分上传等模拟场景。
- `small-dark-en.png`：880×560、深色英文，生产 React + 脱敏 IPC。
- `verification/browser-*.json`：改前/改后浏览器回归；`scripts/dev/verify-node-probes.mjs` 可复现。
- `verification/native.json` / `native.py`：macOS 真实 mihomo 1.19.32 / sing-box 1.14.2；TCP 成功而 HTTPS 失败、HTTP 204 成功而 UDP 黑洞、取消和父进程消失回收、固定公网测试以及独立业务探针。两个内核都完成 TCP/HTTPS/UDP，以及 4 MiB 下载 / 1 MiB 上传；每个对照/同期业务采样均 30/30 成功，记录实际重叠数与分位数，不据小样本宣称零干扰。
- 本地 Rust 383 通过、8 个显式原生用例默认忽略；原生相关用例已按上述驱动另行执行。Clippy、前端生产构建、47 项前端单测、原有延迟历史和新增分项 UI 回归通过。

未验：原生窗口完整 Tauri IPC/安装产品流程、Windows Job 与 Linux 实机。CI 将随 PR 触发，提交时不宣称已通过；未安装或替换用户客户端、订阅及系统网络设置。
