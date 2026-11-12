# 第三方组件与许可声明

Crossbow 分发、依赖或引用的第三方组件及其许可。各组件版权归其各自作者所有。

## 随安装包分发的组件

| 组件 | 来源 | 许可证 | 分发方式 |
| --- | --- | --- | --- |
| [mihomo](https://github.com/MetaCubeX/mihomo) | MetaCubeX | GPL-3.0 | 未修改的官方 sidecar 二进制，经外部进程调用（不链接）；macOS TUN 变体以 setuid root 运行 |
| [sing-box](https://github.com/SagerNet/sing-box) | SagerNet | GPL-3.0-or-later | 未修改的官方 sidecar 二进制（sing-box 内核引擎） |
| [wintun](https://www.wintun.net/) 0.14.1 | wintun.net 官方构建 | 见 wintun.net 许可条款 | 随 Windows 安装包分发（`wintun.dll`，与内核同目录），供 TUN 使用 |
| geoip.metadb / geosite.dat | [MetaCubeX/meta-rules-dat](https://github.com/MetaCubeX/meta-rules-dat) | GPL-3.0 | 内核运行数据文件，随包预置并自动更新 |
| [Manrope](https://fonts.google.com/specimen/Manrope) 字体 | Manrope 贡献者 | [SIL OFL 1.1](https://openfontlicense.org/) | 桌面端与官网界面字体 |

## 运行时引用（不随包分发）

- [React](https://react.dev/) / [Astro](https://astro.build/) / [Tauri](https://tauri.app/) 及其余开源依赖：各自仓库的许可证（MIT / Apache-2.0 等），详见 `apps/site/package-lock.json`、各 `Cargo.toml` / `Cargo.lock`。

## 说明

- mihomo 与 sing-box 均以**独立 sidecar 进程**运行、经本地控制 API 通信（不链接、不修改），依 GPL-3.0 的聚合（aggregate）边界与本项目的 MIT 代码分离；分发时同时提供上游二进制来源链接。
- 本项目代码本身以 [MIT](LICENSE) 许可证发布。
