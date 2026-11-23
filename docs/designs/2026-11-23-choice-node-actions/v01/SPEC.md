# 选择状态与动作入口的空间分离

日期：2026-11-23。关联：#647。状态：已实现，生产模拟回归/构建/变量门禁通过，原生 WebView / CI 待验。

## 用户场景与决定

用户指出「仅看可用」关闭时左侧像缺失图标；节点对勾离右边太远，且 hover 后被测速按钮替换。核心问题是空位没有解释其用途，状态与动作争用同一位置。

保留 Crossbow 现有字级、明暗配色、描边、圆角和组件，不引入新的整页视觉风格。

| 当前问题 | 正式实现 | 原因 |
| --- | --- | --- |
| ChoiceChip 未选只显示空槽 | 始终显示14px勾选框，未选空心，选中填充+对勾 | 状态可读且不靠颜色独立表达 |
| 选择时字重变化 | 两态统一中等字重 | 保持控件宽度，避免筛选工具栏位移 |
| 节点对勾为隐藏动作向左让位38px | 右上角只放状态，距卡片右边10px | 状态与内边距对齐 |
| hover、focus、测速时隐藏对勾 | 对勾持续显示，增加 aria-pressed | 实际选中状态保持可见/可访问 |
| 测速只在 hover/focus 出现 | 常驻底行尾部，与延迟数据相邻 | 鼠标、键盘和触控均可发现；无空操作槽 |
| 所有节点标题预留50px | 只在确有选中标识时预留24px | 未选节点长名称可用完整宽度 |

勾选框使用既有间距/圆角/描边/主题变量；操作复用公共 Button small 档，28px等高正方形，保留 loading、disabled、焦点及完整标签。底行最小高度28px，长状态截断且完整详情保留在卡片 title。卡片最小/最大高度与分页策略保持现行规则。

## 同类问题巡检

- ChoiceChip 所有消费者一起生效：Proxies 的仅看可用、NodeQuality 的历史/分项视图和目标、NodeProbes 的连通性/吞吐类型。
- 移除 layouts/Layout.css 中历史38px状态偏移与50px标题预留，同时移除全局 layout.css 中 hover/focus 隐藏状态的规则，避免新旧样式叠加。
- Select/MultiSelect 的选择指示列用于菜单对齐；日志行复制区用于防止 hover 时正文跳动，保留这些有明确目的的占位。
- Button 加载时隐藏文字由 spinner 替代，维持动作宽度；Popover/Notification 的进入退出透明度属于显示生命周期，不是静态空槽，保留。

## 生产预览与源稿

基线 `ba3b61dfafd87dd090d2525e1593db418521495b`。所有截图直接渲染生产组件，数据为合成节点/IPC；不操作真实订阅、代理或安装。脚本记录的 revision 为基线 HEAD，改后证据对应本目录 manifest 的工作区源文件哈希，不能把它误认为干净基线截图。

| 场景 | 改前 | 改后 |
| --- | --- | --- |
| 默认1120×734浅色中文、筛选关闭 | [截图](previews/before-light-off.png) | [截图](previews/after-light-off.png) |
| 筛选开启 | [截图](previews/before-light-on.png) | [截图](previews/after-light-on.png) |
| hover 已选节点 | [截图](previews/before-light-hover.png) | [截图](previews/after-light-hover.png) |
| 880×560深色英文、长名称 | [截图](previews/before-small-dark-en.png) | [截图](previews/after-small-dark-en.png) |

[测速等待](previews/testing.png) · [键盘焦点](previews/small-keyboard.png) · [质量历史共享筛选](previews/quality-filters.png)。
可编辑实现与验证脚本见 source.zip，配合仓库依赖使用，不是独立打包应用。

## 验证

从仓库根目录运行（Node24/已安装浏览器与依赖）：

```sh
CB_CHOICE_EVIDENCE=/tmp/cb647 node scripts/dev/verify-choice-node-actions.mjs
CB_QUALITY_EVIDENCE=/tmp/cb647-quality node scripts/dev/verify-node-quality.mjs
CB_PROBES_EVIDENCE=/tmp/cb647-probes node scripts/dev/verify-node-probes.mjs
node scripts/dev/verify-design-tokens.mjs
npm run build --prefix apps/desktop
```

- 新场景保护宽度稳定、对勾不被 hover/聚焦/加载隐藏、右侧无空动作槽、鼠标和键盘测速不切节点、卡片回车选择、最小窗口按钮不越界。
- 质量历史和分项测试生产回归均通过，覆盖目标切换、取消、失败/迟到、空记录、焦点恢复和小窗口。
- TypeScript/Vite通过；原有大 bundle 提示保留。变量门禁通过。
- 原生 Tauri/WebView、其他平台及本次提交 CI 待验；不持续轮询 CI。不把模拟验证当作真实节点测速验收。

改前复现需在基线工作区使用相同脚本并设置 CB_CHOICE_BEFORE=1，当前源码只能重现改后。
