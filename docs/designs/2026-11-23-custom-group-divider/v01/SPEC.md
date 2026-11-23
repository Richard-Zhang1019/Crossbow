# 自定义组列表尾线修复 v01

关联 #641，基线 main 8626039。已实现，生产组件模拟与变量消费门禁通过；原生窗口及本 PR CI 待验。

## 根因与选择

CustomGroups 使用公共 components/Modal；列表态明确传 footer=null。多余横线来自 CustomGroups.css 的 .cb-custom-list-row 每行都带 border-bottom，只有一个组也会画在弹窗正文末尾。

| Before | After | Why |
| --- | --- | --- |
| 每行都有下边框，末行也显示尾线 | 末行 border-bottom=0，其他行保持分隔 | 分隔线仅表达列表项关系，不制造无操作底栏的视觉边界 |
| 可能误以为公共 Modal 的 footer 有问题 | 公共 Modal 不改；标题和编辑保存底栏保留 | 避免影响其他弹窗和编辑操作区 |

**最佳方案：局部移除末行尾线。** 相比全局删除 Modal 边框或给正文遮挡补丁，这一方案直接修复线的所有者，不影响其他使用者。

## 验证与复跑

```sh
CB_GROUP_EVIDENCE=/tmp/custom-group-divider node scripts/dev/verify-custom-groups.mjs
node scripts/dev/verify-design-tokens.mjs
```

- 改前/改后均为实际生产 CustomGroups/Modal，使用同一隔离 IPC 场景，无真实订阅或系统代理操作。1120×734浅色中文截图复现/修复单行尾线；多行仅保留两条行间线；880×560深色英文、说明展开也不出现尾线。
- 原场景继续覆盖空列表、工具条等高、筛选预览、失败留草稿、重试保存、来源失效与编辑操作区。编辑 footer 不因列表修复消失。
- 低影响局部样式修改，没有重新跑全量构建或后端测试，不将此说成原生验收。既有自定义组脚本已经在 CI 注册，扩展它而不另建一套测试入口。
- 截图页脚版本为固定测试标识；源码由 manifest SHA256固定。没有改公共 Modal 或网络逻辑，没有替换本地安装。

source.zip 保存可编辑生产 CSS、回归场景与生产组件源稿；历史档案不覆盖。规范见 UI-DESIGN.md 与 DESIGN-SYSTEM.md。
