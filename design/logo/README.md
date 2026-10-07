# Crossbow Logo 资源

完整十字弓 Glyph（45° 构图：弩身 / 弓臂 / 弓弦 / 箭），源图为参考图抠图版。

## 源文件

| 文件 | 说明 |
|---|---|
| `crossbow-logo-extracted.png` | 抠图源 · 黑色 Glyph + 透明底（864×864，保留抗锯齿） |
| `crossbow-logo-white.png` | 白色版 + 透明底 |

## App Icon

### 最终版（黑底 + 紫罗兰→靛→蓝渐变）
- 背景 `#0A0A0F`，渐变 `#8B5CF6 → #6366F1 → #3B82F6`
- `app-icon-gradient-v3-1024.png` 高清源
- `app-icon-gradient-v3-512.png` / `app-icon-gradient-v3/`（512 / 256 / 128 / 64 / 32）

### 主题变体（黑底 + 青→蓝渐变）
- 渐变 `#22D3EE → #3B82F6`
- `app-icon-theme-gradient-512.png` / `app-icon-theme-gradient/`

## 官网 Favicon（favicon/）

基于最终版 App Icon 生成：

```
favicon.ico                    # 多尺寸打包（16 / 32 / 48 / 64 / 256）
favicon-{16,32,48}.png         # 现代浏览器
favicon-{192,256}.png          # PWA / Android
apple-touch-icon.png           # iOS 书签（180×180）
```

HTML 引用：

```html
<link rel="icon" href="/favicon.ico" sizes="48x48 64x64 256x256">
<link rel="icon" type="image/png" href="/favicon-32.png" sizes="32x32">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
```

## 托盘图标

### macOS（tray-macos/）
Template 模板图：单色剪影 + 透明底，系统自动适配深 / 浅菜单栏。

| 文件 | 尺寸 |
|---|---|
| `tray-black-{16,22,32,44,66}.png` | 黑色（**推荐**，template 模式标准） |
| `tray-white-{16,22,32,44,66}.png` | 白色（手动模式，深色菜单栏） |

Tauri 用法：

```rust
TrayIconBuilder::new()
    .icon(tauri::image::Image::from_bytes(include_bytes!(
        "../design/logo/tray-macos/tray-black-32.png"
    ))?)
    .icon_as_template(true)   // 系统自动反色
    .build(app)?;
```

### Windows（tray-windows.ico）
主题青色剪影（`#22D3EE`），深 / 浅任务栏均可见。
多尺寸打包：256 / 64 / 48 / 32 / 24 / 16。

## 预览

`preview-menubar.png` — 托盘图标在深 / 浅菜单栏中的实景效果。
