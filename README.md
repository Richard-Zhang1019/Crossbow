# Crossbow

开源代理客户端（macOS + Windows + iOS）：mihomo / sing-box 双内核，三端共享 Rust
「大脑」，目标「Clash Verge 的易用性 + Surge 级可观测性」。当前处于 M0（macOS MVP），
Windows 为 M1.5、iOS 为 M2；不做 Linux。

- 设计文档：[docs/DESIGN.md](docs/DESIGN.md)
- MVP 范围：[docs/MVP.md](docs/MVP.md)
- 线框图：[docs/WIREFRAMES.md](docs/WIREFRAMES.md)
- 迭代路线图：[docs/ROADMAP.md](docs/ROADMAP.md)

## 仓库结构

```
crates/crossbow-core   共享核心：Profile/覆写链/配置渲染/快照存储（三端共用）
apps/desktop           桌面端（Tauri 2 + React + TS；macOS 现役，Windows M1.5 同壳）
apps/ios               iOS 端骨架（M2：SwiftUI + PacketTunnel 扩展，XcodeGen）
docs/                  设计/范围/线框
```

## 开发

依赖：Node 20+、pnpm、Rust stable（仓库内 `rust-toolchain.toml` 自动选择）。

```bash
pnpm install
pnpm dev            # 前端热更（仅 webview）
pnpm tauri dev      # 完整桌面应用
cargo test -p crossbow-core   # 核心库单测
```

内核生命周期集成测试（可选，需要 mihomo 二进制）：

```bash
curl -sL -o /tmp/mihomo.gz https://github.com/MetaCubeX/mihomo/releases/latest/download/mihomo-darwin-arm64-v1.19.32.gz
gunzip -f /tmp/mihomo.gz && chmod +x /tmp/mihomo
CROSSBOW_CORE_BIN=/tmp/mihomo cargo test -p crossbow-desktop --lib -- --test-threads=1
```

桌面应用开发时指定内核二进制（不设置则用 `~/Library/Application Support/com.crossbow.app/binaries/mihomo`）：

```bash
CROSSBOW_MIHOMO_BIN=/tmp/mihomo pnpm tauri dev
```

iOS 端（M2）：需要完整 Xcode + Apple 开发者账号，见 apps/ios/README.md。

Windows 端（M1.5）：同一 Tauri 壳，需要 Windows 10 1809+（WebView2）与 Rust
`x86_64-pc-windows-msvc` 工具链；打包用 NSIS。

### 本地环境备注

- 本仓库通过 `rust-toolchain.toml` 固定 Rust stable；若 rustup 下载超时，可临时用镜像：
  `RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup rustup toolchain install stable`
- 若链接报 `tapi error: unknown architecture arm64e.x1`（CLT 与最新 SDK 不匹配），构建时指回旧 SDK：
  `export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`
  （装好完整 Xcode 并 `xcode-select` 切换后可移除。）

## 许可

GPL-3.0（与 mihomo 内核一致；内核以 sidecar 独立进程方式分发，不链接）。
