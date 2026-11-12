# 发版流程

## 正式 macOS 发布

正式产物统一由 `.github/workflows/release.yml` 生成，继续使用 Tauri 的 Developer ID 签名、公证与更新器。`scripts/release-macos.sh` 仅校验或调用此流程，不再本地重新签名后上传旧的更新归档。

1. 在仓库根目录运行交互式版本更新脚本：
   ```bash
   python3 scripts/ci/set_release_version.py
   ```
   脚本会显示当前应用版本，接收新稳定版本号（`X.Y.Z`，可带 `v` 前缀），预览并确认后更新 `Cargo.toml`、`Cargo.lock` 中所有 Crossbow workspace package（包括服务包）、`apps/desktop/package.json`、`apps/desktop/src-tauri/tauri.conf.json`，以及中英文 README 的当前版本和下载链接。它保留历史发布说明、依赖版本和 root `package.json`（该文件是私有工作区工具包元数据）。添加 `--dry-run` 可只预览文件范围，不写入。
2. 确认并提交版本变更后，离线校验（Python 3.11+）：
   ```bash
   VERSION=vX.Y.Z bash scripts/release-macos.sh --check
   ```
3. PR 通过并合并后，在明确准备发版时创建、推送对应的稳定版本 tag：
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
   tag 推送会自动触发 release；无需再运行 dispatch。
4. 需要重试尚未发布的版本时，使用 Actions 的 Re-run jobs，或在对应 tag 的检出目录执行：
   ```bash
   VERSION=vX.Y.Z bash scripts/release-macos.sh --dispatch
   ```
   dispatch 要求远端 tag 存在、指向本地 HEAD，且本地已跟踪文件无未提交改动。脚本不自动提交、打 tag 或推送。已有 Release 拒绝覆盖；失败发布若留下不完整 Release，应先人工检查处理，不要改写已发布 tag。

### CI 门禁

- 开始构建前检查凭据是否存在，解析 tag 的不可变 commit，校验源码版本一致。
- 复用 build 工作流，在同一 commit 上运行脚本回归、workflow 静态检查、双平台 core clippy/test、桌面后端测试与 unsigned 构建。
- PR/分支 CI 取消旧运行；正式发版不取消正在运行的同版本任务。
- Tauri 完成应用与 DMG 签名、应用公证；DMG 再独立提交公证和装订。
- 校验实际 `.app` 的版本、主程序和 sidecar 执行权限、arm64 架构、签名、公证票据和 Gatekeeper 接受状态。
- 只读挂载 DMG，将其中 `.app` 与更新归档逐文件比较（含符号链接、执行权限）；解压更新归档后再次验证签名、公证票据。
- 在临时 GitHub-hosted macOS runner 上，分别启动 DMG 和更新归档中的应用，用空 HOME/XDG 数据目录观察 15 秒；启动即退出或输出致命错误（包括弹出致命错误对话框）会阻止发布并上传日志。此检查不代表 WebView 已交互就绪，也不覆盖 UI 卡死、旧配置迁移、TUN 或实际联网，仍需人工验收。
- `latest.json` 必须匹配版本、平台、更新 URL 和签名文件；最终只发布检查后的文件，并附 `SHA256SUMS`。此处检查签名元数据一致性，更新器签名的密码学校验仍由 Tauri 更新器完成。
- 完整获取历史和 tag，使用当前 tag 的最近祖先 tag 生成中文更新日志。

### Secrets

在 GitHub → Settings → Secrets and variables → Actions 配置：

| Secret | 用途 |
|---|---|
| `APPLE_CERTIFICATE` | Developer ID Application 的 base64 `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` 导出密码；空密码证书可留空 |
| `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID` | Apple ID、App 专用密码和 Team ID，用于公证 |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri 更新器私钥；当前按空私钥密码配置 |

证书导入和临时钥匙串由 Tauri bundler 管理，避免重复手动导入造成签名阻塞。凭据只存在检查不验证证书有效期/权限；实际签名与公证步骤会验证。

### 失败诊断与下载校验

- DMG 公证失败时保存提交结果，并按 submission id 获取 Apple 的详细日志；应用公证失败见 Tauri 构建步骤输出。
- `release-diagnostics-*` / `startup-diagnostics` artifact 保留 7 天；验证后的发布产物保留 30 天。
- 更新 sidecar 版本时，必须同步更新 `scripts/ci/download_sidecar.py` 的目标文件名、SHA-256 和 `src-tauri/src/engine/download.rs` 的版本。摘要来自上游对应 Release 的资产 digest；镜像也必须通过同一摘要校验。
- CI macOS `.app` 使用 tar 上传，以保留执行权限与符号链接；它是未正式签名的测试产物。
- 下载全部发布文件后可执行：`shasum -a 256 -c SHA256SUMS`。SHA-256 便于检测文件损坏，不替代 Developer ID 和更新器签名。

## Windows

正式 `release.yml` 在双平台检查通过后构建 Windows x64 NSIS 安装程序及更新器签名，由 macOS 发布任务统一附加到 Release，并写入 `latest.json` 与 `SHA256SUMS`。可选 `WINDOWS_PFX_BASE64` / `WINDOWS_PFX_PASSWORD` 用于 Authenticode，未配置时发布未签名安装包；更新器签名不等同于 Windows 代码签名。Windows 发布使用系统级 NSIS 安装器，同时打包服务与 wintun。

PR 的 build 工作流上传 NSIS 测试包，并在临时 Windows runner 上验证服务安装、升级、启停及拒绝非 GUI 调用。Windows 编译与后端测试通过 Actions 验证，GUI 和真实联网需按 [Windows 验收说明](WINDOWS-TESTING.md) 在实机测试。

## Linux（v0.11.5 起提供实验性 x86_64 压缩包）

`release-linux` 在 Ubuntu 24.04 上从同一不可变 tag commit 构建桌面程序与服务，使用经过摘要校验的 mihomo。Linux 打包显式关闭更新器产物，不加入 `latest.json`；macOS 和 Windows 的更新签名流程保持原有行为。

`package_linux.py` 检查三份 ELF 的 x86_64 架构、必需文件和执行权限，归档后逐项核对内容与权限。压缩包包含 `crossbow-desktop`、`mihomo`、`crossbow-service`、`run.sh`、服务安装脚本、polkit action 和 `README.txt`。安装服务仍需用户明确执行 sudo，解压或启动桌面不会自动提权。

Linux job 失败会阻止整个 Release 发布。通过的归档汇入统一的 `SHA256SUMS` 和 Release 下载列表。发布包需要 GTK3、WebKitGTK 4.1、AppIndicator，glibc 基线为 Ubuntu 24.04；不是自包含 AppImage，也不声称支持旧发行版或完成 niri/Wayland 实网 TUN 验收。升级服务前须关闭 TUN，待空闲服务退出，再运行新包内的安装脚本同时替换服务和内核。

本地打包脚本测试：`python3 -m unittest discover -s scripts/ci/tests -v`；真实 Linux 构建由 Actions 验证。运行及网络验收见 [Linux 网络接管](LINUX-NETWORK.md)。

## iOS

后续通过 App Store / TestFlight 分发，NE entitlement 申请通过后配置。
