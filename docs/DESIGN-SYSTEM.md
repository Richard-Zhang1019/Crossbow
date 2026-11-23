# Crossbow 界面设计系统

状态：#639；桌面生产 CSS/TSX 的当前消费规范。品牌、Frame Studio 外壳和已确认交互沿用 UI-DESIGN.md；本文件负责基础视觉语言和变量契约。新增页面先选择语义用途，再选择公共组件，不凭视觉感觉添加一个新像素值。

## 一、变量分层和适用范围

唯一基础入口：`apps/desktop/src/styles/tokens.css`，由 index.css 在基础样式之前载入。层次为基础刻度 → 语义用途 → 组件内部状态。页面只消费变量与公共组件，不能在业务页重新定义主题或给 Button 上色。

1. 基础刻度：`--cb-space-8`、`--cb-leading-20-px` 等，保留现有精确值。修改基础刻度会影响全部使用位置，必须审计引用。
2. 语义用途：`--cb-type-body`、`--cb-text-dim`、`--cb-space-panel` 等，新增设计优先选这一层。
3. 组件：`--cb-control-height`、`--cb-radius-modal`；Button 的 `--button-*` 是共享组件内部状态钩子，不是页面重新设计按钮的入口。

本轮迁移桌面全部 27 份消费 CSS，并处理 JSX 内字号、字体工具类和进程头像色板。没有把网站和 SwiftUI 假装变成 CSS 消费方：它们应遵循相同语义，通过各自 token 适配层单独迁移。SVG 路径、真实国旗/品牌图片、图表数据、坐标、分页测量和断点不属于字体/颜色规范值。

## 二、字号与字体：按任务分配

| 用途 | 变量 | 当前值 | 何时使用 |
| --- | --- | --- | --- |
| 页标题 | --cb-type-page | 20px | 每页主要名称；不用于 Modal 或每张卡的标题 |
| 区块标题 | --cb-type-section | 16px | 页面内主要分组/状态结论、Modal 标题；不用于普通按钮 |
| 正文 | --cb-type-body | 13px | 表单名称、表格主信息、列表名称、常规正文 |
| 次要信息 | --cb-type-secondary | 12px | 描述、按钮、辅助数值、表格次行、操作说明 |
| 图注/元信息 | --cb-type-caption | 11px | 时间、单位、徽标、轴标签等低密度短内容；不能拿来装下长正文 |
| 默认控件 | --cb-type-control | 12px | 公共按钮，保持全局规格 |

字体 `--cb-font-ui` 使用系统无衬线栈；代码、IP、日志使用 `--cb-mono`。数字比较使用 tabular-nums，不用字体大小暗示成功/失败。不得用过小文字解决空间不足，应换行、折叠辅助说明或使用明确的内容滚动区。

字重：regular=400（正文）、medium=500（控件/列表标题）、emphasis=550（现有强调层）、semibold=600（分组/重要数据）、strong=650（少量既有强调）。选择性使用，不将整张表加粗。

行高优先 `--cb-line-body`=20px、`--cb-line-caption`=16px、`--cb-line-title`=28px；长说明可使用基础比例 1.5/1.6。已有表格的固定行高/密度不要通过全局替换行高破坏。字距通常继承；特殊数字/品牌的 tracking 变量属于组件规范。

### 兼容项不是新增字号表

`--font-xs/sm/md/lg/title` 是旧基础刻度，现有语义变量指向它们；消费方已改为语义名。`--cb-type-compat-*` 和 `.cb-legacy-text-*` 仅保留原有 10/10.5/11.5/14/18 及大数据字号的外观，不能作为新增普通正文的选项；门禁通过 design-token-compat.json 的引用上限阻止继续扩散（减少兼容引用允许，增加需显式评审）。新增数据大字必须以组件用途定义别名并评审，例如统计总量不能影响正文层级。此次精确抽离不声称已重新设计所有历史字号。

## 三、颜色：先看含义，再看主题

| 角色 | 变量 | 适用场景 | 禁止误用 |
| --- | --- | --- | --- |
| 主文字 | --cb-text | 标题、名称、正文、需要识别的值 | 不用纯黑/纯白硬编码 |
| 次文字 | --cb-text-dim | 说明、次行、表格标签 | 不等于不可用 |
| 弱元信息 | --cb-faint | 时间、单位、辅助脚注 | 不能隐藏错误或重要字段 |
| 强调 | --cb-accent / accent-soft / accent-line | 链接、选择、焦点及弱选中底色/边框 | 不能代表“正常运行” |
| 成功 | --cb-ok | 已验证成功/运行结果 | 尚未测试、未观测路径不能标绿 |
| 警告 | --cb-warn | 受限、需关注、可恢复问题 | 不把全部说明染成警告 |
| 错误 | --cb-bad | 失败、破坏性操作语义 | 仍须文字/图标说明，不仅靠红色 |
| 品牌 | --cb-brand | Logo 当前主题墨色 | 不跟随蓝紫强调色 |
| 主操作 | --cb-primary / --cb-on-primary | 由公共 Button 使用 | 页面不自行配按钮前景/背景 |
| 流量 | --cb-down / --cb-up | 下载/上传图表和图例 | 与好坏无关，不等同成功/失败 |

背景层级：窗口 --cb-chrome → 内容 --cb-bg → 面板 --cb-surface → 悬停/抬高 --cb-surface-2；浮层 --cb-floating，遮罩 --cb-overlay。普通边界 --cb-line，控件边界 --cb-line-strong。默认用一条分隔线，避免重复边框/多层卡片。

明暗主题由 :root / :root.dark 集中管理，业务代码不分支填写十六进制颜色。头像 `--cb-avatar-N-bg/fg` 是稳定身份色，不是网络状态色。固定暗色编辑器的历史变量、精确透明度及已有阴影保留原行为，新增主题面板应使用角色色。

## 四、间距、形状与控件

- 主要节奏：4 / 8 / 12 / 16 / 24 / 32px，分别用于紧凑图标、控件内、相邻字段、语义分组、面板和页面留白。优先 --cb-space-control=8、--cb-space-section=16、--cb-space-panel=24；基础刻度还保留现有微调值，避免机械改布局。
- 宽度/高度随实际可用空间分配；最小间距和图表上限仍按 UI-DESIGN.md。设计间距变量不能替换 ResizeObserver 测量结果。
- 圆角：control=7、card=10、panel=12、modal=16px；圆形头像用 circle，状态胶囊用 pill。组件原有 compact 等精确刻度保留，页面不任意加一种圆角。
- Button：默认高32、compact高28、toolbar高34px，用语义变量；同排同职责操作用同规格。表单输入、选择器、开关复用统一组件，页面只调整布局。
- 图标默认16、小图标12、连接路径32px。图标缺省跟随 currentColor，状态图标与状态文字共用角色色。传入图标组件的几何 size 不等于新的正文档位。
- 分隔线使用 --cb-stroke-1（1px）；焦点轮廓 --cb-stroke-2（2px），与错误/选中含义区分。轮廓必须可见，不能为了去掉“多一道 border”而全局禁用键盘焦点。

## 五、状态、动效和层级

hover 使用 surface-2 或组件自己的状态变量；pressed/selected/disabled/loading 各有对应语义。disabled 使用公共透明度 --cb-opacity-disabled，不只换鼠标形状。loading 保留尺寸和文字布局，防止左右抖动。

反馈常用 --cb-duration-feedback=140ms 与 --cb-ease-out；基础 duration 还保留既有场景时长。只 transition 有需要的属性，不用 transition:all；键盘不增加延迟，尊重 prefers-reduced-motion。转圈周期不是弹窗进出场时长，不混用。

z-content=1、raised=10、overlay=40、modal=90、notification=100、popover=110，使用变量，不在业务页加9999。Modal 嵌套的计算层级是运行时算法，允许基于变量运算。Portal 不受任意父容器 overflow 裁切；Modal/Select/通知各司其职，详见 MODAL.md 与 NOTIFICATIONS.md。

## 六、消费示例与边界

```css
.example-row {
  gap: var(--cb-space-control);
  padding: var(--cb-space-section);
  font-size: var(--cb-type-body);
  line-height: var(--cb-line-body);
  color: var(--cb-text);
  border: var(--cb-stroke-1) solid var(--cb-line);
  border-radius: var(--cb-radius-card);
}
.example-description { color: var(--cb-text-dim); font-size: var(--cb-type-secondary); }
```

允许保留零值、inherit/currentColor/transparent、百分比和布局函数；断点、容器几何、图表/SVG 坐标不强制变成无意义的“像素变量”。组件计算尺寸如头像字号使用 size 参数乘 token 比例。三个运行时 CSS 属性明确登记：--cb-modal-width（公开 width API）、--cb-flow-first-end/second-start（连接路径测量）。不允许以“动态”为名散落一套字号/色板。

新增视觉值先确认有没有语义变量；确需新增时在 tokens.css 定义用途、主题与消费场景，并更新本规范。不要仅生成 --color-123 然后宣称设计已统一。

## 七、执行与验收

`node scripts/dev/verify-design-tokens.mjs` 检查全部桌面 CSS/TSX 的字号、颜色、行高、字重、圆角、阴影、间距、时长硬编码及未定义变量，含拒绝样例。该工具复用 Vite 安装的 PostCSS 解析 CSS，不改锁文件；两个桌面 CI 场景入口均调用。

门禁不等于视觉验收：还应检查真实生产组件的默认/880×560、明暗主题、中英文、长文本、空数据、加载/错误、Tab/Esc/焦点及滚动；语义是否正确仍需评审，脚本无法判断“红色是否误标正常”。新属性或其他技术栈不自动受此门禁保护，应显式扩展。

设计规范的可复跑生产组件图谱：`node scripts/dev/verify-design-system.mjs`，覆盖默认/880px与明暗主题，展示五档字号、文字/状态色和公共按钮/输入/选择器。设 CB_DESIGN_EVIDENCE 可保存截图。

Modal 标题使用 --cb-modal-title-size（16px）/--cb-modal-title-line-height（24px），标题栏上下留白使用 --cb-modal-header-padding-block（12px），窗口高度不超过600px时使用 --cb-modal-header-padding-block-compact（8px）；水平留白仍为24px/20px。关闭按钮保留原点击尺寸并与标题垂直居中。标题继续最多三行，不改变页面标题20px规则（#641 / PR #642 用户补充）。
