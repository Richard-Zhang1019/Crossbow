# 公共 Modal 紧凑标题栏与自定义组尾线 v02

2026-11-23，#641 / PR #642。用户补充要求公共 Modal 标题字号、上下留白一起缩小，追加到原 PR。v01 的列表去尾线保留，本版不覆盖旧稿。

| Before | After | Why |
| --- | --- | --- |
| Modal 复用20px页面标题字号 | 16px区块标题档，24px行高 | 弹窗层级应轻于页面大标题 |
| 标题栏默认上下20px，矮窗口12px | 默认12px，矮窗口8px | 减少标题栏占用，正文获得更多可用空间 |
| 标题和关闭按钮顶部对齐 | 垂直居中，关闭按钮仍保留原点击尺寸 | 单行/多行标题与关闭操作关系一致 |

使用公共 Modal 样式与四个组件语义变量，所有消费者共用；没有对自定义组单独加覆盖，也没有改变页面20px标题。维持三行长标题、完整可访问名称/字符串悬停全文、固定底栏、焦点/关闭保护与正文滚动。原列表末项去尾线、行间分隔规则仍在。

变量：--cb-modal-title-size=16px、--cb-modal-title-line-height=24px、--cb-modal-header-padding-block=12px、--cb-modal-header-padding-block-compact=8px；均别名到既有基础刻度，未新建字号档位。水平留白、正文/底栏留白及 Modal 宽度不变。

## 已验证

- TypeScript/Vite 构建通过，保留已有大 bundle 提示。
- 设计变量门禁通过，无新增视觉字面量或未定义引用。
- 公共 Modal/Select/Notification 生产场景通过：异步、关闭保护、兄弟/嵌套、键盘焦点、长标题/最小窗口/固定操作等。
- 自定义组生产场景通过：空/单/多项、分隔、帮助展开、失败草稿、保存重试、来源失效、明暗中英文与最小窗口、编辑底栏。
- 截图为真实生产组件 + 脱敏 IPC，不改真实代理、订阅、TUN 或安装。原生 WebView及更新提交的CI待验，不轮询。

## 预览与复跑

在已安装依赖和 Chromium 的仓库，用 Node 24：

```sh
CB_MODAL_EVIDENCE=/tmp/modal-header node scripts/dev/verify-modal.mjs
CB_GROUP_EVIDENCE=/tmp/groups-header node scripts/dev/verify-custom-groups.mjs
node scripts/dev/verify-design-tokens.mjs
```

before.png 引用同工作树 v01 已归档的去尾线/旧标题实际预览，基线 f7d6fb1；after.png 及 long-title-dark.png 为本次重跑。所有截图由相同组件和确定性合成数据渲染；测试页脚版本不是发布版本。源稿哈希在 manifest，截图时尚未提交，不能仅凭脚本输出的基线 SHA 当作实际源码版本。
