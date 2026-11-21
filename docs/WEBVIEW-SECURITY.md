# WebView 内容安全策略

关联 Issue #602、PR #620。策略入口是 `apps/desktop/src-tauri/tauri.conf.json`。
这是一项防御加固，基线缺少显式 CSP，并不代表已发现可利用的 XSS。

## 生产资源与边界

| 资源 | 来源与策略 |
| --- | --- |
| 脚本 | Vite bundle 与 index.html 的启动脚本。`script-src 'self'`，由 Tauri 自动补哈希/nonce。禁止任意内联脚本、外部脚本与 eval。 |
| 样式 | 本地 CSS、启动骨架 style 标签与 style 属性、React 动态样式。保留 `style-src 'self' 'unsafe-inline'` 作为无 nonce/hash 时的样式兼容；Tauri 注入 nonce 后，现代引擎忽略这一 unsafe-inline。另设 `style-src-attr 'unsafe-inline'`，只允许样式属性，不放宽脚本。 |
| 图片/字体 | 本地资源与打包后的 data URL，允许 self/data，不允许远程 CDN。 |
| IPC | `connect-src 'self' ipc: http://ipc.localhost`。macOS/Linux 使用 IPC 自定义协议，Windows 使用 ipc.localhost；不扩张 capabilities。 |
| 订阅、内核连接、更新、匿名统计 | Rust 后端负责网络请求，通过 IPC 提供数据，WebView 不直接连接外部服务，不需要加入 connect-src。CSP 也不限制后端请求。 |

Tauri 的编译与运行时会为合法脚本/样式补充来源，不能只把 JSON 字符串当成最终响应策略。
尤其是 `style-src` 出现 nonce/hash 时，其 unsafe-inline 不再允许 HTML 的 style 属性；单独的 style-src-attr 用于保留骨架中的变量与间距。

## 开发策略

`devCsp: null` 会被反序列化为 None，Tauri 会回退到生产 csp，并非禁用 CSP。
本项目明确配置 devCsp：允许 Vite/React refresh 的内联前导脚本和开发样式，以及 localhost/127.0.0.1:5173 的 HMR WebSocket；无 unsafe-eval 或任意外部来源。
生产 csp 不包含开发脚本豁免和 WebSocket 来源。

桌面 devUrl 通常直接由 Vite 提供响应，浏览器预览也不经过 Tauri 的本地资源协议，不能用它们证明生产 CSP 已生效。

## 回归验证

先运行 `pnpm --filter @crossbow/desktop build`，再运行 `node scripts/dev/verify-csp.mjs`。
脚本使用现有 playwright-core 与本机 Chrome/Chromium，在独立 loopback 端口提供真实 dist：

- 模拟 Tauri 的启动脚本哈希和样式 nonce，验证合法资源无 CSP violation、骨架 style 属性保留。
- 动态插入未批准的内联与外部脚本，以及外部 fetch，验证被 CSP 拒绝且未进入网络。
- 将 devCsp 实际作为响应头施加于 Vite 页面，验证 React refresh 前导脚本、应用与样式可加载。

该脚本是手动运行的浏览器回归入口，目前未接入 CI，也不下载浏览器。
IPC 使用脱敏替身。它不替代三平台原生打包验收：

| 平台 | 原生需核对 |
| --- | --- |
| macOS / WKWebView | tauri 自定义协议响应头、IPC，主窗口/通知独立窗口、拖动、深链接导入、更新。 |
| Windows / WebView2 | tauri.localhost / ipc.localhost、IPC，以及同一功能链路。 |
| Linux / WebKitGTK | Tauri 的 HTML CSP meta 注入、自定义协议、IPC，以及同一功能链路。 |

原生受控探针应记录 securitypolicyviolation 事件，确认合法资源不报错、未批准脚本和外部连接被拦截；仅构建成功不等于完成这项验收。

## 新增资源流程

1. 确认资源来自 WebView 还是 Rust 后端，先考虑打包到本地。
2. 必须外连时，仅在对应指令加入必要协议和域名，在 PR 中说明来源与理由。
3. 不加入通配符、生产 unsafe-eval 或生产脚本 unsafe-inline，不关闭 Tauri 的自动 CSP 修改。
4. 重跑浏览器回归，并在受影响的原生平台检查实际响应策略与 CSP violation。

参考：[Tauri CSP](https://v2.tauri.app/security/csp/)、[SecurityConfig](https://v2.tauri.app/reference/config/#securityconfig)。
