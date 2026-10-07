# Crossbow Logo 资源（v3 渐变）

完整十字弓 Glyph（45° 构图：弩身 / 弓臂 / 弓弦 / 箭）· 2026-10-07 由 Codex 重绘。

## 主资产
| 文件 | 用途 |
|---|---|
| `app-icon-gradient-v3-1024.png` / `-512.png` | App 图标母版（圆角深底 + 渐变） |
| `app-icon-gradient-v3/icon-*.png` | 32/64/128/256/512 导出 |
| `glyph-gradient-v3-864.png` | 渐变 Glyph · 透明底（官网 Logo） |
| `tray-template-white-512.png` / `tray-template-black-512.png` | 菜单栏模板形状（512 母版） |
| `tray-macos/tray-{black,white}-*.png` | 16/18/22/32/36/44/66 导出 |
| `favicon/` | 官网 favicon 全家桶（ico + 16/32/48/192/256 + apple-touch） |

## 消费点映射
- **桌面应用图标**：`apps/desktop/src-tauri/icons/`（32/128/128@2x/icns/ico）— 由 1024 母版重生成
- **菜单栏托盘**：`apps/desktop/src-tauri/icons/tray-icon.png`（黑色 44px，`icon_as_template(true)` 系统自适应）
- **官网**：`apps/site/public/`（favicon 全家桶）+ `apps/site/src/assets/logo-glyph.png`（导航/页脚 Logo）
