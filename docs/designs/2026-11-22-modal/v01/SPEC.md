# 统一业务 Modal v01

2026-11-22（北京时间），已实现，待 PR 评审与原生 WebView 验收。关联 #631。

目标：统一 12 个消费文件、14 处业务弹窗；固定标题与底栏，正文独立滚动，失败不丢草稿，最小窗口保持动作可达。沿用 Crossbow 现有主题与 Button，不引入 antd 默认视觉。

方向采用设计知识库的瑞士网格纪律为主、深色科技灰阶为辅，并服从项目已确认规范。通过留白、分隔线与标题/正文层级组织内容，避免新增嵌套卡片；共享底栏动作高度一致，自定义组列表刷新/新增同为 small。

最佳方案：封装现有 Base UI Dialog primitive，提供受控 Ant Design 风格 API。替代方案为仅扩展旧 Dialog，但无法统一其手写焦点、底栏滚动和实例层级。基础交互、尺寸与迁移对照见 [Modal 规范](../../../MODAL.md)。

固定头尾，默认宽 560px，自定义组 680px，节点历史 720px；窗口上下留 24px，小高度压缩内边距。多个实例由统一层级协调，仅顶层拥有焦点和关闭权限；父子同次挂载按父下子上登记。对 Base UI 的初始/持续焦点管理进行兄弟实例交接补充，Select 保留自己的浮层与 Escape。

## 预览与源稿

- `before-preview.png` / `after-preview.png`：main fa13061 基线与本 PR 生产自定义组表单，1120×734 浅色中文。
- `before-auto-small-bottom.png` / `after-auto-small-bottom.png`：880×560 深色英文，长表单底部状态，对照固定操作栏。
- `primitive-light.png` / `primitive-small-dark.png`：生产 Modal + 合成长正文/异步提交夹具，1120×734 浅色 / 880×560 深色。
- `source.zip`：本版本可编辑生产组件、消费方和回归入口快照，依赖完整仓库；`manifest.json` 给出文件哈希。
- `verification/modal.json`：专用交互回归。`checks.json` 区分本地/模拟与待验范围。

已验证：异步失败留输入、成功等待受控关闭、等待关闭保护、嵌套/兄弟 Escape 与遮罩、独立 ARIA ID、Select Escape、双向 Tab 与焦点返回、保留隐藏内容、快速开闭回调、固定 footer；以及既有规则保存、恢复重试、配置包预览释放和分项测试关闭取消等业务。

原生 WebView/完整安装环境与各平台实机待验。没有修改内核、系统代理、订阅或原生安全确认；没有发布、安装或自行合并。CI 提交后触发，不将 PR 创建等同 CI 通过。
