# Crossbow 官网重新设计 · v01

2026-10-28 · 提案 · Issue #174。仅设计稿，尚未替换 apps/site 或部署。

## 任务与边界

不读取、引用旧官网的页面结构、CSS 或文案，依据 main c4ab8cd 的功能与官方 Release 信息重新构思。仅沿用 Crossbow 名称和自有标志。界面片段为本次新绘的功能示意，采用脱敏演示数据，不作为当前 App 的逐像素截图或性能承诺。

## 信息架构与主张

| 章节 | 主张 | 产品依据 |
| --- | --- | --- |
| 首屏 | 开源桌面代理客户端，可看清当前连接路径 | Home、ConnectionFlow、Connections |
| 节点 | 按地区筛选、搜索、延迟排序，逐项测试 | Proxies.tsx |
| 接管 | 系统代理与 TUN 独立开关，可同时开启 | lib.rs set_tun/sysproxy_toggle、tun_windows.rs |
| 监测 | 连接/日志/诊断和四站点检测提供排查线索 | Diagnostics、Connections、Logs、SiteConnectivity |
| 配置 | 订阅/本地配置、定时更新、Merge/Script 覆写、双内核 | Profiles、OverridePanel、ipc.ts |
| 上手 | 安装、导入自有配置、选择节点与接管方式 | import_profile_*、set_active_profile |
| 平台 | macOS Apple Silicon / Windows x64；iOS 开发验证中 | 官方 Release 2026-10-28 查询结果、apps/ios/README.md |

README 当前版本段落过期，文案不机械沿用。Windows TUN 已有当前代码路径，不沿用旧 README 的“尚未开放”；跨平台服务/授权区别在 FAQ 说明。iOS 不提供虚构 App Store/TestFlight 入口。下载统一前往官方 Releases，不写死版本号。

## 视觉方向

采用 design-taste-frontend 抽取的 Colour chapters（seed 1112850316）：各章节以完整的色场切换构成阅读节奏。抽取的糖果配色与代理客户端的状态语义不符，重抽一次后将色场映射为连接蓝、纸白、冷灰、监测深色。保留方向的色块章节和交替构图，移除无产品含义的装饰形状；巨型形体以放大的产品功能演示替代。字体使用 landing-page-design 允许的 Manrope，并为中文提供系统字体后备；字体随稿归档。

概念：Crossbow 的“连接路径”成为整页的阅读路径，依次回答连向哪里、如何接管、如何排查、如何配置。唯一实时交互是接管方式切换，它改变同一条路径的说明，不真的修改用户网络。

页面优先采用 design-taste-frontend 的平面导航、纯色、无静态阴影规则；与 landing-page-design 的玻璃导航/标题渐变有冲突时，不混搭。所有按钮、锚点、FAQ 和移动菜单均可操作，键盘焦点可见，减少动态效果可用。

## 文案与交互

完整文案见 COPY.md。首屏主动作“下载桌面版”，指向平台下载段。顶部 GitHub、FAQ、许可、隐私和使用文档均有真实目的地。演示状态不发出网络请求，未宣称免费节点、订阅服务、客户评价或测速保证。

## 归档与验收

保留 HTML/CSS/JS、界面示意源稿、实际渲染图、桌面/平板/手机预览、manifest 和设计检查记录。改稿使用 v02，不覆盖本版。正式官网确认后在独立实现任务中接入现有构建及发布流程。

## 用户反馈

2026-10-28：用户认为观感不佳，明确要求移除截图或仿应用面板。v01 停止迭代，原样保留为对照；后续见 v02。
