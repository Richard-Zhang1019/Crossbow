# 诊断页改版 v01 · 已实现

> 2026-11-17。Closes #533。预览截图见 [previews/](previews/)（真实 dev 环境实现截图，非概念稿）。

## 背景（#533 诊断）

故障取证卡不可折叠且双记录全量展开，把诊断结果顶出视口；诊断列表没有自己的滚动区（整页滚动，列表沉底只剩一条缝，overlay 滚动条还会浮压「通过/失败」文字）；状态语义四处堆叠；「优先处理」callout 与列表首项重复；探针结果为流式文本、6 个网络接口观测未去重；空状态无行动入口。

## 方案（B 设计系统内，不改调色板/字号/IA）

1. 诊断列表成为唯一滚动区：高度链 `.cb-paper → .cb-page-content.cb-view-diagnostics → .cb-diagnostics-page → .cb-diag-results`（`:has` 选择器下传，旧 WebKit 优雅退化为整页滚动）；列表 `padding-inline-end:12px + scrollbar-gutter:stable`，overlay 滚动条只压留白不压文字；`min-height:150px` 保底可见。
2. 故障取证默认折叠为一行摘要（名称 + 开始时间 + `faultLayerStats` 探针统计 + 上一条 + 状态 pill）；展开详情封顶内滚 `clamp(240px, 42vh, 440px)`——「上一条」再展开也不挤掉列表。
3. 探针「层 × 主探针 × 独立复核」对齐网格 + 正常/失败 pill；观测接口按端点去重为 chips（`dedupeObservedTargets`）。
4. 一条状态带统一结论/故障 chip/检查时间/首个阻塞/动作（动作唯一出口）。
5. 首个失败项列表行内升级（警示左边条 + 优先处理标 + 指南链接），删除重复的优先处理 callout；已通过行不再显示修复按钮（原逻辑漏 `!row.ok`）。
6. 空状态居中 CTA（图标 + 标题 + 说明 + 开始诊断）。

## 实现

`apps/desktop/src/pages/Diagnostics/{index.tsx,report.ts,report.test.mjs}`、`apps/desktop/src/styles/layout.css`（改版样式块追加于文件尾）、`apps/desktop/src/i18n/i18n.ts`（新增 9 键，中英齐全）。数据面与后端无改动。测试：`report.test.mjs` 新增 `faultLayerStats`/`dedupeObservedTargets` 回归用例；CI 同款全量前端测试通过。

## 已知取舍

- `:has` 高度链在旧 WebKit 上失效时，退化为旧的整页滚动行为（列表回到底部沉底，但不劣化于改版前）。
- 短窗（≤620px 高）媒体查询沿用原有紧凑规则，列表保底 150px 优先。
