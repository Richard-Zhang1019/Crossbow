# 语义图标辅助操作 v01

日期：2026-11-23（北京时间）；#643。状态：已实现，待评审与原生 WebView 验收。

## 目标与方向

用户希望配置重命名使用下划线铅笔，局域网复制地址使用复制图标，并统一同类操作。按设计知识库瑞士网格的减法/对齐纪律，服从现有 Crossbow Frame Studio 和新 token 规范；只压缩高频且语义明确的辅助操作，保留主要动作和危险操作的文字。

最佳方案：新增项目 IconAction，组合现有 Button/Icon 与已经安装的 Base UI Tooltip；不逐页自行绘制 hover，不引入额外图标库。相比只把按钮文字换成 SVG，此方案同时覆盖 tooltip、键盘焦点、可访问名称、禁用与加载、反馈及主题一致性。

图形：16px 统一线性描边；重命名/编辑为下划线铅笔，复制双页，刷新回转箭头，上下排序箭头。默认紧凑按钮 28px，常态透明、次要文字色；hover 使用 surface-2/强分隔线；focus/pressed/disabled/loading 复用 Button。复制成功是 cb-ok 勾选；失败沿用统一 Notification，不伪造成功。

Tooltip hover 延迟 350ms，键盘 focus 即显示，portal 避免裁切，使用独立 ID、role=tooltip 与 aria-describedby；禁用/加载收起。复制图标有屏幕阅读器状态反馈。无新增动画，尊重 Button 的减少动效规则。

## 修改清单与保留项

- 配置来源、覆写名称：重命名图标，保留原编辑与业务保存流程。
- 局域网：地址复制及刷新图标，复制失败不显示勾，成功反馈独立。
- 自定义组、进程规则、用户规则：行编辑图标；用户规则排序改为同风格箭头并保留边界禁用。
- 连接详情：字段复制图标，只对刚复制的字段显示勾；底栏“复制全部”仍有文字。
- DNS：复制结果图标，反馈跟随当前记录；保留原摘要脱敏。
- 日志：copy/check 图形替换原复制字样，保留固定占位/hover/focus/title 与读窗冻结，不为每个实时日志行引入 Tooltip 状态。
- 已有的节点/统计等刷新图标继续复用原组件；规则集操作菜单的编辑及新增/保存/测试、删除、状态切换保留文案，不把菜单选项变成猜图标。

## 预览和验证

基线 `825c295def00a0d9825bfd580c6c89d071cbc738`；改前从独立 main 工作树运行同一生产页面+脱敏 IPC 夹具，改后为此基线上的本 PR 工作树实现，实际源文件哈希见 manifest。所有订阅/地址均为示例，不读取用户数据。

`previews/before-*` 与 `after-profiles-light.png` / `after-lan-light.png` 为 1120×734 浅色中文；`after-rename-hover.png`、`after-rename-focus.png`、`after-rename-disabled.png`、`after-copy-hover.png`、`after-copy-success.png` 保留真实状态；`after-lan-dark-small-hover.png` 为 880×560 深色英文；`after-connection-copy-hover.png` 为 Modal 内生产复制浮层。

实际验证：tsc/Vite 构建、47 项前端单测；verify-icon-actions 的真实重命名保存/取消/禁用、hover/focus、复制成功/失败、字段反馈、深色英文小窗口；LAN、进程规则、用户规则、自定义组、DNS 回归和共享连接/日志订阅场景通过。新专项纳入两个 CI 场景入口。

边界：浏览器生产 React + 脱敏 IPC/clipboard。未修改后端网络/系统代理或用户订阅；原生 WebView 与平台实际剪贴板待验，PR CI 提交后触发、不持续等待。此局部前端改动没有重新运行 Rust 网络测试。

可编辑源稿 `source.zip` 包含本次组件、消费方、样式依赖与回归脚本，可在完整仓库依照锁文件运行，不是独立安装包。
