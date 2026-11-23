# Crossbow 分路由使用文档

日期：2026-11-23；关联Issue #651。状态：已实现；本地构建、内容回归和浏览器静态站点验收通过，PR CI / 公开部署待验。

## 用户目标与设计依据

v0.15.0以来增加了独立策略和诊断工具，旧指南16个章节堆在长页。新结构让每个功能有自己的路由与源码文件，左侧目录常驻，右侧阅读当前文档。

使用 design-kb 的「新极简/网格」排版方法与 design-taste-frontend 的内容/任务优先约束，服从项目既有 site.css 品牌：Manrope搭配中文系统字体、既有明暗调色、不引入新视觉主题。对照来源为私人知识库04风格和01中的排版/界面规范索引；外部网站状态沿用库内记录，未作本次实时访问保证。

## 布局及字级

- 保留官网顶栏。桌面文档工作区高为剩余视口高度，左目录和右正文独立滚动；宽屏另有本文目录。
- 目录约220px，正文最大740px，本文目录150px；1280以下收起本文目录，800及以下左目录改为原生details。跳页保持目录滚动位置，并确保当前条目可见。
- 标题32px、小节22px/17px、正文15px、目录13px、辅助12px；手机主标题28px，目录输入16px。字号、字重和间距消费文档变量，色彩消费官网变量。
- 分类为开始使用、配置与分流、连接与网络、观测与排障、维护与恢复。目录可筛标题/简介，区分空结果；不是全文搜索。
- 当前项、目录披露、键盘焦点、Escape关闭及章节锚点明确；手机选择路由后目录收起。标题和正文仅一份，不用隐藏一整页的方式模拟切换。
- 文档页以相邻文档导航和反馈入口收尾，官网其他页面保持原页脚。所有内容无需客户端Markdown解析/SPA依赖，无JS仍有可读文章和导航。

## 路由和维护

`/guide`与`/zh/guide`为概览。原16章节迁为独立Astro路由/内容组件，新增15篇双语Markdown路由；每语言31功能页。共享导航元数据和DocsLayout负责目录、SEO与上一篇/下一篇。每功能有真实路由文件，构建输出完整HTML。

旧 `/guide#id` 和 `/zh/guide#id` 跳转对应功能路由；无JS提供手动旧章节索引。跨语言保持同篇文档，篇内翻译不同的锚点会重置。Markdown文档互链由构建插件补语言和部署前缀。首页/功能页入口更新为直达路由。

功能覆盖与源码依据以 [SITE-CONTENT](../../../SITE-CONTENT.md) 为当前维护标准。特别记录：配置包v2不包含域名/IP用户规则和自定义组；规则/协议/DNS能力不能把上游内核支持误当作客户端全部支持。

## 实际预览

| 场景 | 文件 |
| --- | --- |
| 旧版长页 | [改前](previews/before-long-guide.png) |
| 新版概览 | [浅色](previews/overview-light.png) |
| 功能阅读 | [DNS浅色](previews/dns-light.png) · [英文深色](previews/dns-dark-en.png) |
| 窄屏 | [手机正文](previews/mobile.png) · [手机目录](previews/mobile-menu.png) · [平板](previews/tablet.png) |
| 旧内容拆分 | [覆写示例](previews/override.png) |

所有截图来自真实Astro静态构建，无合成桌面组件、无远程订阅/凭据。基线main `0267e60004846bbf54fa3f97c304449f7d8fa0ce`（v0.15.3发布合并后），改后由manifest记录源哈希。
可编辑源稿见source.zip，作为同一仓库基线上的源码快照使用；保留现有资产。不会覆盖旧设计档案。

## 验证与边界

- 根路径及 `/Crossbow/` 子路径构建均成功；8项回归随网站build执行，覆盖主题启动、双语路由存在性、旧章节、语言/前缀链接转换和可执行覆写示例。
- 浏览器检查62功能页的直接访问及本地链接，无失效项；验证独立title/canonical/hreflang、固定目录与正文滚动、筛选空态、章节/翻页/后退、同篇语言切换、旧hash迁移、明暗、平板、390/360px无横溢出和无JS内容。
- 72个全站静态页面由Astro生成，sitemap随构建更新。新文档无额外第三方运行时依赖。
- 浏览器验证使用本机已装Chrome，静态构建由本地HTTP服务器承载；不等同于真实手机Safari、公开Netlify/GitHub Pages或PR CI验收。
- 未修改桌面/内核功能，不运行真实代理/TUN或更换用户本地应用；公开部署和CI待验，按约定提交后不轮询。

从仓库根目录使用已有依赖：

```sh
npm run build --prefix apps/site
CB_SITE_EVIDENCE=/tmp/site-docs node scripts/dev/verify-site-docs.mjs
SITE_BASE=/Crossbow/ npm run build --prefix apps/site
SITE_BASE=/Crossbow/ CB_SITE_EVIDENCE=/tmp/site-docs-base node scripts/dev/verify-site-docs.mjs
```

重现旧布局需要在基线工作区构建并设置CB_SITE_BEFORE=1；当前源码只生成新版。浏览器证据见verification目录。
