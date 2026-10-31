# Crossbow

Crossbow 是面向 macOS 和 Windows 的开源代理客户端，使用 Tauri 2、React 和 Rust 构建，支持
mihomo 与 sing-box 双内核。它把订阅、节点选择、系统代理、TUN、连接与日志放在同一个界面，
并在托盘 / 菜单栏显示实时速率。

**当前版本：** [v0.12.1](https://github.com/Richard-Zhang1019/Crossbow/releases/tag/v0.12.1)。
提供 macOS Apple Silicon DMG、Windows x64 安装包和 Linux x86_64 实验性压缩包。
Windows 版持续完善中，TUN 使用安装包附带的受控服务；实机兼容性问题请附版本、通知详情与诊断结果提交 Issue。

Linux 正在面向 **CachyOS x86_64 + niri/Wayland** 分阶段适配。显式 HTTP/SOCKS 代理可独立使用；
第二阶段增加了需单独安装和授权的 mihomo TUN 服务，遇到其他代理、VPN 或策略路由会拒绝接管。
Linux 发布包附带桌面程序、mihomo 和可选 TUN 服务安装脚本；系统代理、AppImage/DEB 和自动更新仍未开放。构建及实机记录见 [Linux 测试说明](docs/LINUX-TESTING.md)，
服务安装、权限和恢复边界见 [Linux 网络接管](docs/LINUX-NETWORK.md)。

## 安装与使用

| 平台 | 安装包 | 说明 |
|---|---|---|
| macOS Apple Silicon | [下载 DMG](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.12.1/Crossbow_0.12.1_aarch64.dmg) | 拖入「应用程序」后启动；Developer ID 签名与公证 |
| Windows x64 | [下载安装程序](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.12.1/Crossbow_0.12.1_x64-setup.exe) | 安装后启动；尚无 Windows 代码签名，请核对官方来源及校验文件 |
| Linux x86_64（实验性） | [下载 tar.gz](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.12.1/Crossbow_0.12.1_linux_x86_64.tar.gz) | 解压后以普通用户运行 `run.sh`；依赖及 TUN 安装见包内 `README.txt` |

下载与更新内容以 [GitHub Releases](https://github.com/Richard-Zhang1019/Crossbow/releases/latest) 为准；
应用内可在「设置 → 软件更新」检查更新，下载后请按界面提示完成安装。
发布文件附有 `SHA256SUMS`；`main` 可能包含尚未发布的改动。

1. 导入订阅链接或配置文件，在「代理」页选择节点；也可在托盘菜单切换节点并查看延迟。
2. 日常使用可在首页开启系统代理。需要接管不读取系统代理的应用时，macOS 可在「设置」中开启 TUN。
3. 本应用的 TUN 与系统代理可同时开启、独立控制；TUN 首次开启需要管理员授权。
   请先关闭其他代理或 VPN 的 TUN，避免路由冲突。

v0.12.1 修复下载进度通知卡 14% 数字块的垂直居中，并改进 Dock 图标的色彩管理加载。
Windows GUI 与真实网络仍需实机验收，步骤见
[Windows 测试说明](docs/WINDOWS-TESTING.md)。显式代理能访问而浏览器不能访问时，请检查浏览器是否使用系统代理。

macOS 测试网络配置时可先阅读 [TUN 离线恢复说明](docs/TUN-RECOVERY.md)；恢复命令需要仍在运行的备用代理。
macOS 配置和日志位于 `~/Library/Application Support/com.richard.crossbow/`。旧版
`com.crossbow.app` 数据会自动迁移，升级前仍建议备份数据目录。

## 开发

依赖 Node.js 20+、pnpm 和 Rust stable；macOS 本地构建还需 Xcode Command Line Tools。

```bash
pnpm install
pnpm tauri dev
pnpm --filter @crossbow/site build
```

本地打包桌面应用（不生成需要发版签名的更新器产物）：

```bash
cd apps/desktop
npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

仓库结构：

```text
crates/crossbow-core  共享的配置、覆写与存储逻辑
apps/desktop          macOS / Windows 桌面应用
apps/site             官网（Netlify）
apps/ios              iOS 工程骨架
docs                  设计、路线图与恢复说明
```

桌面开发时可用 `CROSSBOW_MIHOMO_BIN=/path/to/mihomo pnpm tauri dev` 指定本地内核。
详细设计、范围与计划见 [设计文档](docs/DESIGN.md)、[MVP](docs/MVP.md) 和
[路线图](docs/ROADMAP.md)；macOS 正式发版步骤见 [发版文档](docs/RELEASE.md)。

## 许可

项目声明采用 [GPL-3.0](https://www.gnu.org/licenses/gpl-3.0.html)。mihomo 与 sing-box
作为独立内核进程运行，分别遵循其自身许可。
