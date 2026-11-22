# 应用内 Modal（#631）

统一入口 `@/components/Modal`，使用现有 Base UI Dialog 与 Crossbow Button/主题。
API 风格参考 [Ant Design Modal](https://ant.design/components/modal/)，实际行为以本文为准；没有引入 antd。

```tsx
<Modal open={open} title="编辑" onCancel={() => setOpen(false)}
  onOk={async () => { await save(); setOpen(false); }}>
  <Input value={draft} onChange={e => setDraft(e.target.value)} />
</Modal>
```

| 属性 | 默认与契约 |
| --- | --- |
| open / title / children / onCancel | 必填；完全受控，关闭请求不自行改 open |
| onOk | 可选，支持 Promise；缺省确认按钮禁用；同步锁防重复，等待时阻止用户关闭 |
| okText / cancelText | 当前语言的保存 / 取消 |
| confirmLoading | false；外部等待，确认 loading 且关闭保护 |
| okButtonProps / cancelButtonProps | 项目 Button props；组件保留默认按钮的 onClick 与等待保护，禁用条件合并 |
| footer | undefined：默认确认/取消；null：无底栏；ReactNode：自定义固定底栏 |
| width | 560，支持数字像素或 CSS 字符串；最大窗口宽减 32px |
| centered | true；false 时靠窗口顶部，保留 24px 边距 |
| closable / keyboard | true；分别控制关闭按钮及 Esc |
| mask.closable | true；false 禁止点击外部关闭 |
| closeDisabled | false；业务提交期间阻止 Esc、遮罩、关闭按钮及默认取消 |
| destroyOnHidden | true；关闭卸载正文；false 保留隐藏内容与输入状态 |
| afterOpenChange / afterClose | 底层完成开闭后调用；初始关闭不触发 afterClose；调用方直接卸载组件时不会补发回调 |

确认成功由业务修改 open；失败不会自动关窗。裸 Promise 异常经统一 Notification；已有 notify.run 的业务继续自己处理结果。每次确认拥有独立通知标识。
自定义 footer 的按钮仍由业务负责重复提交保护、禁用/等待与成功关闭。组件不拦截业务按钮。需要 form 提交时用唯一 form id 和 Button 的 form 属性关联，底栏不放在滚动正文中。

单个或嵌套弹窗的焦点、背景交互隔离、页面滚动锁由 Base UI 负责；共享层级协调还覆盖兄弟实例，仅最顶层响应 Esc/外部关闭并拥有焦点锁。每个标题有独立 ARIA ID。嵌套 Select 仍使用现有 primitive，Esc 先收起 Select。标题与 footer 固定，正文是唯一垂直滚动区；日志摘要的 pre 不再产生第二个纵向滚动区。

保持当前条件挂载方式，关闭后的业务卸载清理仍执行。不要把任务取消、会话释放、复制反馈清理搬进 Modal。外部强制改变 open 不受 closeDisabled 阻止，业务仍负责应用退出/导航时的任务处理。

## 迁移对照与审计

| 消费方 | 迁移与生命周期 |
| --- | --- |
| Profiles 来源导入 | 固定导入/文件底栏，busy 时阻止关闭；原兼容性预检与快捷键保留 |
| Profiles 配置包导入/导出 | 固定操作底栏；pkgBusy 关闭保护，原 closeImportPackage 释放预览会话 |
| OverridePanel 新建 | 固定操作底栏；creating 保护，失败保留名称 |
| Connections 详情 | 只读正文 + 复制反馈/复制全部底栏；清理复制反馈不变 |
| Logs 导出预览 | 固定导出底栏、导出期间关闭保护；脱敏数据不变 |
| Diagnostics 摘要 | 固定复制底栏；关闭继续 clear 复制反馈 |
| NodeQuality（含分项测试） | 720px；历史隐私控制固定底栏，正文单独滚动；NodeProbes 卸载取消保留 |
| CustomGroups 列表/编辑 | 680px；编辑固定删除/取消/保存底栏，提交关联原 form；列表刷新/新增同为 small 规格 |
| ProcessRules / UserRules / RuleSets / SceneRules | 固定原表单操作底栏；显式 form 关联，busy 关闭保护，保留既有原子保存和错误草稿 |
| ConfigRecovery | 列表恢复动作保留，busyId 非空关闭保护；可连续恢复与失败重试 |

main fa13061 的 12 个消费文件、14 处原 Dialog 全部迁移，旧实现与旧全局样式删除。全仓前端检索无手写业务 mask/role=dialog、window.confirm/alert 遗留。Select、Popover、通知不是 Modal，不迁移。
后端保留原生深链导入确认（外部来源安全边界，可能主窗口尚未就绪）和启动前致命错误（无可用 React 根）；Windows 深链 TaskDialog 同理。退出失败安全兜底保留原生能力，不转成普通结果提示。本 PR 不修改后端逻辑或系统文件选择/授权交互。

没有新增静态 Modal.confirm/info/success/error 或独立 React root。业务有明确受控入口，暂不需要 Provider 确认队列；普通结果继续 Notification。

## 回归

`node scripts/dev/verify-modal.mjs`：生产 Modal/Select 的合成业务夹具，验证异步失败留草稿、成功等待受控关闭、重复提交/关闭保护、嵌套及兄弟层级、ARIA、Select Esc、双向 Tab、焦点恢复、隐藏保留、默认/最小窗口与固定底栏。已纳入两处 CI 场景列表。
既有 custom-groups、process-rules、user-rules、rule-sets、scenes、package-policies、node-probes 和 verify-scenarios 继续保护业务；仅更新旧组件 CSS 选择器，不削弱业务断言。生产构建和前端单测检查 API 与按钮统一性。
这是浏览器生产组件+脱敏 IPC 验证；原生 WebView、系统对话框及完整安装环境待实机验收，不以模拟替代。
