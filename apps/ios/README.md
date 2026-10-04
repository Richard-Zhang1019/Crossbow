# Crossbow iOS 端（M2 · 骨架说明）

> 当前目录是 M2 的工程骨架：SwiftUI App + PacketTunnel 扩展，
> 用 [XcodeGen](https://github.com/yonaskolb/XcodeGen) 从 `project.yml` 生成 `.xcodeproj`。

## 架构（与 DESIGN.md §3 对应）

```
Crossbow.app (SwiftUI)
├─ CrossbowApp            TabView: 首页 / 代理 / 配置 / 更多
├─ TunnelManager          NEVPNManager 封装：安装/启停 VPN Profile、状态监听
├─ crossbow-core (Rust)   UniFFI 静态库：订阅/Profile/覆写/渲染（与 macOS 共享）
└─ App Group 共享容器      渲染后的 sing-box JSON 配置 + 运行状态

PacketTunnel.appex (NEPacketTunnelProvider)
├─ PacketTunnelProvider   实现 startTunnel/stopTunnel，读取 App Group 配置
└─ libbox (sing-box)      进程内核心库（SFI 同款），承载实际代理流量
```

## 前置条件

1. 完整 Xcode（`xcodebuild` 可用；当前机器只有 CLT，需要安装 Xcode）。
2. Apple Developer Program（PacketTunnel 扩展需要付费账号签名）。
3. libbox：从 sing-box release 下载 iOS xcframework，放入 `Frameworks/`（见下），或用 SPM binary target 引入。
4. 生成工程：`xcodegen generate`。

## 目录规划（生成工程前）

```
apps/ios/
├─ project.yml              # XcodeGen 定义（本目录已有）
├─ Crossbow/                # 主 App（SwiftUI）
│  ├─ CrossbowApp.swift
│  └─ TunnelManager.swift   # TODO(M2)
├─ PacketTunnel/            # 扩展 target
│  ├─ PacketTunnelProvider.swift
│  └─ Info.plist
└─ Frameworks/              # libbox.xcframework（gitignore）
```

## 注意事项

- App 与扩展共用 App Group：`group.com.crossbow.app`（project.yml 已声明）。
- iOS 上不允许子进程，core 只能以库形态链接；mihomo 无官方移动库，iOS 内核固定为 sing-box。
- VPN Profile 首次安装需要用户在系统弹窗中确认；上架审核参照 Shadowrocket/Stash 先例准备说明材料。
- `goRuntimeDir`（libbox 运行时目录）必须设置在 App Group 容器内。
