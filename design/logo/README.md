# Crossbow Logo 资源

完整十字弓 Glyph（45° 构图：弩身 / 弓臂 / 弓弦 / 箭）。

## SVG 矢量源文件
| 文件 | 用途 |
|---|---|
| `crossbow-app-icon-theme.svg` | App Icon · 主题配色（深蓝底 + 青→蓝渐变 + 白箭） |
| `crossbow-app-icon-light.svg` | App Icon · 浅色底（箭为深藏青） |
| `glyph-gradient-transparent.svg` | 渐变 Glyph · 透明底（官网 / 文档用） |
| `tray-template-white.svg` | macOS 菜单栏模板 · 白色（深色菜单栏） |
| `tray-template-black.svg` | macOS 菜单栏模板 · 黑色（浅色菜单栏） |

## PNG 导出（png/）
- `app-theme/` `app-light/`：512 / 256 / 128 / 64 / 32
- `tray-white/` `tray-black/`：512 / 44 / 36 / 32 / 18 / 16（透明底）
- `glyph-gradient-512.png`

## macOS 托盘使用
Tauri 里用 template image：把 `tray-template-white.svg` 转出的 PDF/PNG
命名为 `tray-icon.png`（Template 后缀或 `icon.asTemplate = true`），
系统自动适配深/浅菜单栏。
