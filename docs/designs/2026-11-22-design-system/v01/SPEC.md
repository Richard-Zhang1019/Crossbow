# Crossbow 设计系统 v01

状态：已实现并完成本地/生产组件模拟检查，原生窗口和新 PR CI 待验。关联 #639。基线 main bfc7583（含 #638）。

正式规范见 [DESIGN-SYSTEM.md](../../../DESIGN-SYSTEM.md)；包含字号/字体/字重/行高、文字与状态色、背景层级、间距、圆角、控件、边框/焦点、动效、层叠和消费边界。沿用已确认 Frame Studio，设计知识库仅用于层级与网格纪律，没有改品牌和主题。

| Before | After | Why |
| --- | --- | --- |
| 只有五档字号及色板，缺少使用语义 | 基础刻度、语义用途、组件规格三层，职责表与示例集中维护 | Agent 先选内容职责，再选变量 |
| 27 份消费 CSS 散落视觉常量，JSX 另用 Tailwind 字号和头像色板 | 字体、间距、圆角、阴影、透明度、时长、描边等消费变量；既有值精确抽离 | 避免各页重复调整同一种控件 |
| 未定义变量可悄悄回退，旧字号可继续增加 | 生产源码门禁、拒绝样例、旧字号引用上限，接入两处 CI | 防止字面量和非标档位再次扩散 |

## 范围与限制

本次迁移桌面生产 CSS/TSX，不把网页或 SwiftUI 自动算作已迁移。尺寸测量、图表坐标、断点、SVG/国旗资源等属于几何或素材，允许按实际布局消费，不盲目抽成设计刻度。旧字号以兼容变量保留外观，新增普通正文仅用五档语义；兼容引用只能减少，增加需显式评审。

输入值/通知、网络/内核/保存逻辑未改变。生产预览脚本使用真正 Button/Input/Select，示例数据留在开发边界。

## 可复跑

```sh
node scripts/dev/verify-design-tokens.mjs
node scripts/dev/verify-design-system.mjs
node scripts/dev/verify-modal.mjs
node scripts/dev/verify-scenarios.mjs
npm run build --prefix apps/desktop
node --test $(rg --files apps/desktop/src -g '*.test.mjs')
```

- 门禁扫描 88 个桌面 CSS/TS/TSX 源文件，检查当前视觉值规则及未定义变量；这不代替语义审查，不宣称任意新属性和外部技术栈自动受控。
- 构建、47项单测、桌面10场景、公共 Modal/Select/Notification 回归通过；规范图谱覆盖1120/880明暗，桌面场景覆盖中英文/最小窗口。
- 默认浅色 Modal 改前/改后截图字节一致；深色小窗口经像素对比仅71像素不同，范围(x1,y1)-(x129,y31)，位于背景 Open/Retained 按钮交互态。不能从抽样对比推论所有页面逐像素一致。
- 原生 Tauri/WebView、Windows/Linux实际窗口及 CI 待验。本次没有修改真实网络、订阅或本地安装。Vite 既有大 bundle 提示保留。

`source.zip` 保存可编辑生产样式、修改的TSX、门禁、图谱夹具及规范。manifest记录实际源码哈希；工作树截图的base revision不单独代表最终源稿。图谱由生产组件生成，不是独立手绘效果。旧稿不覆盖。
