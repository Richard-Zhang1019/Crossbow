# Dock 图标尺寸修复 v02

日期：2026-10-31。状态：已实现，原生渲染验证通过，正式 App 待更新验收。关联 #246，跟进 #241 / #242。

## 原因和修复

运行时通过 NSApplication.applicationIconImage 直接加载图标，保持了已确认的 #3F52CF，但也跳过 IconServices 给系统图标添加的缩进。原始图标含描边约占画布 99%，因此与相邻 App 相比过大。这是上一版直接加载引入的尺寸回归。

只新增 Dock 专用导出：同一 B1 图形整体缩放到 82%，平移到 (9, 9)，左右上下透明留白相同。底板含描边约占画布 81.18%，不含描边 80.36%。所有图形连同底板一起缩放，不单独缩小 C 标志，不改变圆角比例和颜色。

应用内、官网、Windows、iOS、Finder 使用的原有资源保持不变。Dock 专用 ICNS 仍含多分辨率表示，每个 PNG 嵌入 sRGB ICC。避免运行时 lockFocus 重绘或单一小尺寸位图导致色差、模糊。

## 正式代码

- `scripts/generate-brand-icons.cjs` 从既有图形导出 `design/logo/current/dock-icon.svg`，并用 Tauri 官方工具生成 `apps/desktop/src-tauri/icons/dock-icon.icns`。
- `apps/desktop/src-tauri/src/macos_icon.rs` 在既有 Ready 生命周期加载专用 ICNS。
- `scripts/verify-brand-icons.cjs` 检查资源哈希、每个 PNG 的 ICC、实际不透明边界、居中和品牌色。

## 实际原生验证

`previews/native-comparison.png` 是 AppKit 在同一 96pt 画布、2x 像素下渲染的修复前、系统备忘录、修复后对比。它是原生渲染对比，不是用户 Dock 截图。

`previews/runtime-check.txt` 使用与正式实现相同的 NSApplication.applicationIconImage 路径，在独立进程读回图标，再以显式 sRGB 渲染：32/64/96/128 像素画布下底板宽度分别 26/52/78/104，四边居中；64/96/128 下主色均为 RGB(63,82,207)。

`previews/smoke-window.jpg` 为独立验证程序的实际窗口。程序只显示图标，不启动 Crossbow 内核、不触碰代理、TUN 或配置；运行后已退出。未替换或重启当前正式 App，最终真实 Dock 观感仍需安装新包后由用户确认。

## 复现

```sh
CROSSBOW_SHARP_MODULE=/path/to/sharp node scripts/generate-brand-icons.cjs
swift docs/designs/2026-10-31-dock-color/v02/native-probe.swift apps/desktop/src-tauri/icons/dock-icon.icns --runtime --assert-brand --assert-size
swift docs/designs/2026-10-31-dock-color/v02/native-comparison.swift apps/desktop/src-tauri/icons/icon.icns apps/desktop/src-tauri/icons/dock-icon.icns /tmp/dock-size-comparison.png
cargo check --locked --lib --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo test --locked --lib --manifest-path apps/desktop/src-tauri/Cargo.toml
```

自动检查：70 项品牌资产验证通过；Rust 编译通过，后端 102 项通过、1 项既有忽略。未新增依赖。旧版 v01 及其人工色彩确认记录保留。
