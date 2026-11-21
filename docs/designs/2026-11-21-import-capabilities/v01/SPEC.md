# 导入兼容性：本地预检与协议能力表 v01

日期：2026-11-21。关联：#599，分支 `codex/599-import-capabilities`。状态：已实现，本地/生产组件模拟/隔离原生连接通过；待 PR 评审，未发布。

## 场景与最佳方案

用户粘贴分享链接或批量文本，在保存前知道当前内核能否保留输入的语义；不支持时不生成一个看似成功但不能使用的来源。

候选一是仅扩展前端协议提示和禁用按钮，改动较少，但绕过界面的文件、深链接、HTTP 导入及检查后的引擎变化仍可落库。**最佳方案**是后端复用公开解析器和真实渲染器提供只读预检，保存事务内再次校验，前端只消费状态和能力数据。这样避免两套判定逻辑，并保护所有共用入口的旧状态。

## 正式实现

- `apps/desktop/src/pages/Profiles/ImportCompatibility.tsx` 与对应 CSS：导入对话框内的兼容性段落，不增加独立卡片/警示横幅。
- `apps/desktop/src-tauri/src/commands/profiles.rs`：只读 IPC 返回当前引擎、基线版本、数量、协议代码与能力行，不返回节点详情或 URI。
- `apps/desktop/src-tauri/src/commands/helpers.rs`：候选渲染和来源发布处于同一 Store 事务，预览与提交之间切换引擎仍不能绕过。
- `crates/crossbow-core/src/share_links.rs`：保留 Hy2 userpass 及 TUIC 整体编码认证语义。

## 布局与交互

复用当前 Modal、Button、textarea、主题字体和分隔线，兼容信息置于输入与操作区之间。默认只展示当前内核版本及一句状态；能力表在“查看协议与版本支持范围”下按需展开，避免最小窗口被静态说明撑满。表格使用中性分隔线，不添加重复主按钮或成功背景。

输入后 300ms 本地检查；输入/内核变化时立即禁用确认，忽略旧请求。URL 状态明确区分“下载后检查”和已通过预检；后者也注明尚未测试真实连接。检查异常走统一 Notification 并允许重试；最终保存失败保留输入。文件入口继续使用共用后端预检，不伪装为已经完成粘贴预览。

## 截图与来源

| 文件 | 代码与数据 | 尺寸 / 主题 / 语言 |
| --- | --- | --- |
| [改前](previews/before.png) | main 基线 ea79f13 的 Profiles/i18n 生产源文件，同一合成输入 | 1120×734 / 浅色 / 中文 |
| [改后](previews/light.png) | 本分支生产 React 组件 + IPC 桩 | 1120×734 / 浅色 / 中文 |
| [展开能力表](previews/expanded.png) | 本分支生产 React 组件 + IPC 桩 | 1120×734 / 浅色 / 中文 |
| [小窗口](previews/dark-small.png) | sing-box 不兼容状态 | 880×560 / 深色 / 英文 |
| [检查失败](previews/failed.png) | 检查错误及重试 | 880×560 / 深色 / 英文 |

截图由 `verify-import-capabilities.mjs` 渲染正式组件，不是手绘。界面中的订阅、内核版本和页脚由测试桩提供，不表示该版本桌面安装已验收。改前截图在隔离 worktree 临时使用基线源文件生成并立即恢复，不修改用户工作目录。

## 验证与边界

- Core：174 项单元及 DNS/订阅/导出结构集成测试通过。新夹具按 v2rayN、NekoBox 固定导出源码及 Hysteria 官方规范重建，未运行这些应用抓取实际导出，见[夹具来源](../../../../crates/crossbow-core/tests/fixtures/share-links/README.md)。
- 桌面后端：同步 main bdefbcf（含 #622）后 345 项通过，含导入相关 6 项。覆盖预览成功后引擎变化、失败时旧内存/磁盘不变及 I/O 重试。
- TypeScript/Vite、43 项前端测试、10 项既有生产组件场景、导入专项交互、桌面/core Clippy 与格式检查通过。
- [原生结果](verification/native.json)：macOS arm64，mihomo 1.19.32 和 sing-box 1.14.2，6 组真实 TCP/UDP 往返、4 组错误条件拒绝。均为隔离回环服务，临时 CA 仅供测试进程，没有修改系统信任/代理/TUN。
- 公网节点、原生桌面端到端点击、真实导出应用、其他平台/版本尚未验收；不得将模拟截图或配置预检概括为这些场景通过。

## 复现

在仓库安装前端依赖并使用 Node 24：

```sh
CB_IMPORT_EVIDENCE=/tmp/cb-import-evidence node scripts/dev/verify-import-capabilities.mjs
python3 scripts/dev/verify-share-links-native.py --mihomo /path/to/mihomo --singbox /path/to/sing-box --evidence /tmp/cb-share-native.json
```

`source.zip` 保存本版可编辑实现与验证源文件，文件哈希见 manifest。详细支持范围见 [SHARE-LINKS.md](../../../SHARE-LINKS.md)。
