# 统一通知（Issue #66）

重要操作结果、错误和后台提示使用统一 Notification；可直接感知的成功操作在页面内反馈，不额外通知。
主窗口内顶部居中展示；主窗口隐藏、最小化或被轻量模式销毁时，Linux 使用系统通知，macOS/Windows 使用独立的无标题浮层。
通知数据由 Rust 保存，页面切换或 WebView 重建不会清空队列。

## 设计与生命周期

- 成功 3 秒、信息 5 秒、普通警告 8 秒自动关闭；错误和进度不自动关闭。
- 需要处理的警告使用 `duration: 0`，保留至关闭。
- 最多显示 2 张，错误优先，其次警告、进度、普通结果。排队中的通知不消耗时长。
- 悬停、键盘焦点、展开详情时暂停倒计时。切换通知承载窗口不重置剩余时间。
- 相同 operationId 或 dedupeKey 更新同一卡片；重复内容合并次数。
- 关闭进度卡只是隐藏展示，不取消任务。后续成功不重新弹出，失败会重新显示。
- 被关闭的相同后台故障不反复弹出；新的用户操作可重新显示其进度。
- 会话内保留最近 50 条记录；未处理错误优先保留。待处理队列最多 30 条。
- “最近通知”位于顶部铃铛下方。历史只存在进程内，不写入配置或磁盘。
- 浅色/深色使用现有 cb 主题变量；支持键盘、读屏和减少动态效果。
- 前端任务进度统一使用 `components/ProgressRing.tsx`，配合百分比或阶段说明；禁止新增原生 `<progress>` 长条。更新下载、测速、内核下载等进度均由同一个通知组件渲染。

## 前端用法

```tsx
import { notify } from "../notifications/service";

const result = await notify.run(
  "setting-flags",
  t("settings.flags"),
  () => ipc.setFlagEmoji(next),
);
if (result.ok) setFlags(next);
```

`notify.run` 返回 `{ ok: true, value }` 或 `{ ok: false }`；默认延迟显示进度并报告最终结果，直接操作使用下方的 `error-only` 模式。
失败已经展示，调用方不得继续提示成功或提交乐观状态。需要时重新读取后端真实状态。
独立操作使用不同的标识，例如 `override-save-${id}` 与 `override-rename-${id}`。

```tsx
notify.start("latency-group-a", t("proxies.testing"));
notify.publish({
  kind: "progress",
  operationId: "latency-group-a",
  source: "latency-group-a",
  title: t("proxies.testing"),
  progress: 60,
});
notify.success(t("proxies.testAll"), "latency-group-a");
// 或 notify.error(error, "latency-group-a");
```

`notify.publish` 可指定 description、details、duration、dedupeKey 和 actions。
动作只有明确实现的白名单：settings、profiles、diagnostics、logs、retryDownload。
下载失败提供安全的“重新下载”，其他失败可前往设置或诊断；不要为不可重入的操作添加重试。

页面不要再自行保存 toast/flash/ErrorBar。表单可以保留字段定位和校验关联；
诊断结果表格、实时流量、版本资料、节点状态仍然是页面内容，不应变成大量通知。
页面卸载时的退订、可选数据的缺省值可以静默处理；用户操作和真正的后台故障不可空 catch。

## Rust 与窗口行为

`src-tauri/src/notifications.rs` 提供会话级队列、脱敏和结构化 `app://notifications` 快照。
`Notice::new` 使用稳定 code，展示时由前端按当前语言解释。技术原因放 details，标题保持简短。

```rust
let mut notice = notifications::Notice::new(
    notifications::Kind::Warning, "tun.disabled", "tun-health",
);
notice.duration = Some(0);
notice.actions = vec![notifications::Action::Diagnostics];
notifications::publish(&app, notice);
```

系统代理/TUN 切换、后台更新下载、内核下载的结果由 Rust 直接报告；调用页面关闭后也能完成通知。
后端拥有通知的操作，前端不要再包一层会重复报结果的通知任务。

主窗口准备好并可见时才向它投递；macOS/Windows 后台惰性创建 `notifications` WebView，接收方先监听再读取快照。
仅使用同源本地页面。一次只激活一个通知承载位置，关闭主窗口不关闭通知服务。
macOS 使用 `NSWindow.orderFrontRegardless`，Windows 使用 `SW_SHOWNOACTIVATE`；
不能用会激活窗口的普通 show() 来显示后台通知。动作按钮由用户主动点击后才打开主窗口。
窗口关闭/重建时移除 ready 标记，暂停队列倒计时，直到新接收方就绪。

Linux 后台通过 `org.freedesktop.Notifications` 发送一条可替换的通用提醒，不创建普通 toplevel，避免 Wayland 合成器激活浮层。系统提醒只包含当前语言的“Crossbow 有新通知 / 打开 Crossbow 查看详情”，不发送标题、详情、URL 或内部标识；详细卡片、动作与历史仍由原 Rust 队列和前端负责。用户点击默认动作才打开主窗口，回到前台后撤回系统提醒。

用户关闭系统提醒时只收起它对应的批次，不标记已读或恢复；批次 token 防止迟到的关闭事件隐藏新失败。系统提醒请求 8 秒过期（最终由桌面决定），过期不反复提醒，原卡片等待下次前台展示。重复故障、进度更新和同批次换序不重复投递。通知服务不可用时保留队列并每 5 秒重试，不回退到会抢焦点的窗口。D-Bus 调用在独立线程执行，RPC 超时 300ms；只接受实际服务 owner 和当前通知 ID 的动作/关闭信号，真正退出时最多等待 1 秒撤回。

管理员授权、文件选择、破坏性操作确认仍使用系统交互。
UI 尚未初始化的致命启动错误保留原生兜底；操作系统崩溃报告需要另外排查崩溃原因。

## 安全与性能

- 所有标题、正文和详情在进入 Rust 队列时脱敏；历史和复制使用同一份数据。
- URL 只保留 origin，避免订阅 URL 路径或查询参数泄漏密钥；隐藏 Bearer、secret、token、password 等值。
- 文本限长，队列和历史有上限。不要把完整配置或原始凭据作为通知标识。
- 下载通知进度限频；快照使用 revision 发送，空闲轮询不会持续发送整份历史。
- WebView 创建从后台线程发起，避免 WebView2 主线程死锁；AppKit 原生显示调用只在主线程执行。调用窗口 API 前释放队列锁。

## 验收路径

1. 在设置中输入无效端口，确认顶部错误卡片出现、页面不变高；展开详情、复制、关闭。
2. 保存普通设置，确认成功静默。触发重要成功通知，确认约 3 秒关闭；悬停或把焦点移到通知按钮，确认计时暂停。
3. 测试代理组，确认同一通知从进度变成结果；节点失败时显示警告而不是全部成功。
4. 连续触发多个独立错误，确认不互相覆盖；重复触发同一错误，确认合并计数。
5. 下载更新，确认进度更新；模拟下载失败，确认可重新下载，失败通知不会被成功替换。
6. 展开“最近通知”，确认可查看已关闭通知与脱敏详情；切换页面后历史仍在。
7. 隐藏或最小化主窗口，从托盘触发操作，确认后台提醒不抢焦点、不关闭托盘菜单；Linux 首次与再次提醒均不应创建 `notifications` 窗口。
8. 开启轻量模式并关闭窗口，触发托盘错误；Linux 点击系统提醒打开主窗口，再在卡片进入诊断，macOS/Windows 可直接点浮层“打开诊断”。确认窗口重建与历史保留。
9. 切换深浅主题及语言，检查通知、历史、按钮；检查窄窗口、长错误、键盘和减少动画设置。

静态/单元检查：前端 `npm run build`；Rust `cargo check`；
`cargo test --lib --manifest-path apps/desktop/src-tauri/Cargo.toml notifications::`。
Windows 编译和构建依照仓库规范使用 GitHub Actions。

## 通知中心与降噪（Issue #196）

通知用于用户需要知道的重要结果与需要处理的问题。界面能直接确认的成功操作不再浮出，也不写入历史：主题、语言、普通设置、节点/模式/配置切换、重命名、开关等。失败始终经过统一服务报告，并重新读取实际状态；静默成功不等于吞掉错误。

### 正式交互

- 顶部铃铛显示未读小圆点。面板宽 408px，距铃铛底部 8px；距窗口边缘至少 16px。最大高度 590px，随可用高度收缩；只让列表内部滚动，标题、筛选和底部工具不滚动。
- 采用紧凑行，取消历史卡片套卡片。待处理问题置顶，其余按最后更新时间排列；显示最近发生时间和合并次数。
- `readAt`（已读）、`dismissed`（关闭浮层）、`resolvedAt`（问题恢复）独立。打开中心不自动全读；查看详情/操作单条标记已读，“全部已读”不会解决问题。
- “待处理”只包含未恢复的错误/警告。问题恢复后在同一记录显示已恢复，保留脱敏技术详情。订阅自动/手动刷新成功、TUN 实际健康检查恢复、内核恢复均更新原问题。
- “清除已完成”保护未恢复错误/警告与进行中任务，提供 10 秒撤销。撤销只恢复历史，不重播浮层、不覆盖清除后到达的新结果；待处理筛选下禁用清除。
- 打开中心时隐藏并关闭同一批浮层展示，不取消任务；之后的新故障依然可以通过原生后台浮层报告。不会把中心打开状态持久化到后端。
- 进度卡关闭后任务继续，成功不重现、失败会重现。默认任务延迟 800ms 才展示进度，短任务不闪卡。最多同时展示 2 张浮层。
- 同一后台故障按稳定 operationId / dedupeKey 合并，即使技术详情变化也更新同一记录；显式重试使用新 attempt 标识，失败可以再次出现。
- 会话历史严格最多 50 条，优先淘汰已完成。极端情况下超过 50 个不同未处理问题时，仅保留最近问题；通知中心不是永久审计日志。
- 结构化 `code` / `titleKey` 在展示时按当前语言解析；技术详情和配置名称保留原文。不要把当前语言生成的通用提示写进长期记录。

### 直接操作的前端模式

```tsx
const result = await notify.run(
  "setting-theme",
  t("settings.theme"),
  () => ipc.setTheme(next),
  undefined,
  { feedback: "error-only", titleKey: "settings.theme" },
);
if (result.ok) setTheme(next);
else await load();
```

`feedback: "error-only"` 成功时静默并关闭该操作之前的问题；失败仍发布通知。
没有此选项的 `notify.run` 用于导入/安装等有意义的异步任务，进度延迟 800ms，保留完成结果。
前端手动串联的操作恢复可调用 `notify.resolve(source)`；Rust 使用 `notifications::resolve(&app, source)`。
只有确认真实成功才能调用 resolve；启动 TUN 开关成功不等于连通性已验证。
系统代理/TUN/托盘模式/配置切换的例行进度和成功由后端统一降噪，失败的唯一归属仍在后端。

### 新增验收

1. 切换主题/语言/普通设置：界面变化、无成功浮层和历史；模拟保存失败仍显示错误并恢复真实值。
2. 连续触发同一后台故障：一行、计数更新、最后时间变化；已关闭重复故障不重弹。显式重试失败可重弹。
3. 全部已读后待处理数量不变；清除只移除已完成项，撤销不重播旧浮层。
4. 关闭下载浮层后继续下载；打开中心可看同一任务，完成后仍在同一行。中心打开时无重复浮层。
5. 在 880×560、窄宽度、深色、英文和长详情下检查：无横向溢出、头尾固定、详情可复制、Esc 返回铃铛。

归档设计与正式组件验证见 `docs/designs/2026-10-29-notifications/`。
