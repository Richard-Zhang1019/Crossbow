# Dock Logo 色彩一致性修复 v01

状态：代码已实现；原生自动验色及独立测试程序的实际 Dock 视觉验收均通过。Issue #241，跟进 #228 / #229。基线 `37f303b`（v0.12.0）。

## 目标

保持已批准 B1 形状、#3F52CF 主色和浅色图标底板，解决应用界面与运行中的 macOS Dock 图标色差。此处没有重新设计 Logo。

## 证据与边界

1. 本机 v0.12.0 安装包 ICNS 与基线仓库哈希相同，内部 PNG 已有 sRGB 标签。界面 CSS 的 --cb-brand 也为 #3F52CF。
2. macOS ImageIO 和 NSImage 直接读取安装包 ICNS：64/96/128px 原生绘制的主色为 (63,82,207)。通过 NSWorkspace 获取同一 App 的 bundle icon，则得到偏浅且模糊的图案。
3. 重新注册 App、重载 Dock，以及使用新建有效测试 bundle，均未消除差异。重新导出带统一 ICC 的资源后，静态图标服务路径仍复现。说明只刷新 Dock 或给 PNG 补标签不足以解决本机现象；这里不声称已定位 IconServices 内部具体的缓存/合成算法。
4. 修复路径使用 NSImage(data:) 保留完整多尺寸 ICNS，在 Tauri RunEvent::Ready / AppKit 主线程设置 NSApplication.applicationIconImage。原生验色工具对同一 API 的返回图像执行 64/96/128px 主色断言，均通过。
5. native-comparison.png 为原生 API 的离屏渲染对比，并非用户真实 Dock 的截图。smoke-window.jpg 为独立测试窗口实际截图。用户已比较测试窗口与新增测试图标，确认「颜色一致，也更清晰了」。此验收针对独立测试程序的实际 Dock 图标；正式 Crossbow 包尚未替换或重启。

## 人工验收

2026-10-31，用户确认独立原生窗口与其实际 Dock 测试图标「颜色一致，也更清晰了」。记录见 `previews/manual-verification.json`。该结果补足此前离屏验色无法覆盖的 Dock 最终合成环节；不等同于已安装正式 Crossbow 修复包。测试程序现已退出。

## 实现

- apps/desktop/src-tauri/src/macos_icon.rs：内嵌与 bundle 一致的多尺寸 ICNS，通过原生解码保留 ICC，避免将小尺寸 window icon 的 RGBA 数据放大后用于 Dock。没有新增依赖、修改系统全局图标设置或清空系统缓存。
- lib.rs：在 Ready 回调设置运行时图标；无法解码/不在主线程时保留系统默认图标并记录非致命诊断，图标问题不阻止应用启动。
- generate-brand-icons.cjs：所有品牌 PNG 写入 sRGB ICC；Tauri 导出 ICNS/ICO 后重新给嵌入 PNG 写入同一 ICC，维护容器长度/目录偏移。生成后自动运行品牌校验。
- brand-icon-containers.cjs：保留非 PNG 原生记录，校验容器边界；规范 ICNS 记录顺序，消除上游 hash-map 顺序导致的无意义二进制差异。
- verify-brand-icons.cjs：检查所有品牌 PNG、8 个 ICNS PNG 子图及 ICO PNG 子图的 ICC 一致性，检查尺寸、目录边界、主色、批准的 B1 几何形状及 manifest 哈希。
- 重新导出 68 项品牌资源。49 个 PNG 的 RGBA 像素保持一致；3 个桌面小尺寸 PNG 由原 SVG 规范重新生成。主设计与品牌主色不变。

## 验证

- macOS cargo check --locked --lib 通过；后端测试 102 passed / 1 ignored（既有忽略项）。保留既有 windows_port_listeners dead_code 警告。
- 68 项品牌资源校验通过，连续两次重新导出哈希相同。
- ImageIO 可解码 10 个 ICNS 表示，色彩空间均为 sRGB。
- 原生 AppKit 验色断言通过，结果保留在 previews/runtime-icon.txt。
- 本次没有发布版本、安装新 Crossbow 包或切换代理/TUN。独立验色窗口只加载图标；关闭窗口即退出该测试程序。
- Windows/iOS 本次仅验证静态导出资源与格式；macOS 原生运行时设置通过 cfg 隔离。

## 复现

需要项目依赖及 Sharp（本次使用 Node 24、Sharp 0.35.5）；可通过 CROSSBOW_SHARP_MODULE 指定已有 Sharp 安装。

```sh
node scripts/generate-brand-icons.cjs
node scripts/verify-brand-icons.cjs
cargo test --locked --lib --manifest-path apps/desktop/src-tauri/Cargo.toml
swift docs/designs/2026-10-31-dock-color/v01/native-probe.swift apps/desktop/src-tauri/icons/icon.icns --runtime --assert-brand
swift docs/designs/2026-10-31-dock-color/v01/native-comparison.swift /Applications/Crossbow.app apps/desktop/src-tauri/icons/icon.icns /tmp/crossbow-native-comparison.png
```

native-probe.swift、native-comparison.swift 是可编辑验色源稿；dock-smoke.swift 是独立 AppKit 视觉检查窗口，使用 swiftc -parse-as-library 编译后放入带 icon.icns 和标准 Info.plist 的临时 .app 中即可运行。source.zip 冻结相关实现、源图形、导出资源、配置及验色工具；manifest 记录文件哈希。全部使用公开品牌资产，不包含订阅、代理凭据或用户桌面截图。
