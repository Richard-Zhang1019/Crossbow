# Windows 安装与签名说明

Windows 发布流程支持可选的 Authenticode 签名（#82）；是否启用取决于发布环境是否配置证书。未配证书时产物为未签名安装包。自签名证书未经本机信任时，仍可能显示「未知发布者」或 SmartScreen 提示。更新器签名与 Authenticode 是独立机制。

## 安装步骤

1. 从 [Releases](https://github.com/Richard-Zhang1019/Crossbow/releases/latest)
   下载 `Crossbow_x.y.z_x64-setup.exe`。
2. 浏览器可能提示「此文件可能危害计算机」——点 `...` → **保留** → **仍然保留**
   （Crossbow 全程开源，安装包哈希见 Release 资产 SHA256SUMS，可自行核对）。
3. 双击安装器，SmartScreen 弹出「Windows 已保护你的电脑」时：
   点 **更多信息** → **仍要运行**。
4. 按安装器引导以管理员权限安装（默认 `C:\Program Files\Crossbow`，可自定义到任意 NTFS 路径），安装器会顺带注册 `CrossbowService` 特权服务

> **安装位置说明（2026-11-09 更新，#378）**：安装器默认安装到 `C:\Program Files\Crossbow`，**可自定义到任意 NTFS 路径**（如 `D:\Crossbow`）。安装器会对安装目录做 ACL 硬化（SYSTEM 与管理员完全控制、普通用户只读+执行），保证特权服务二进制在自定义路径下同样不可被普通用户篡改——此前「仅限 Program Files」的限制自 v0.13.10 起移除。注意两点：
> 1. **安装目标必须是 NTFS 卷**：exFAT/FAT32 等无 ACL 支持的卷无法保护服务二进制，安装器会明确报错拒绝。
> 2. **跨版本升级时安装目录可能回退到默认的 C 盘**——静默升级依赖注册表中记录的上次安装目录，安装模式或注册表键位变化会导致读取不到自定义目录。已安装在自定义目录的用户升级前建议先确认新版本落盘位置，旧目录确认不再使用后手动删除。
   （开机自启）——TUN 模式依赖它以管理员权限运行内核，安装一次即授权一次，
   日后开关 TUN 不再弹任何窗口。

## 可选：信任签名证书（消除「未知发布者」安装警告）

信任我们的公开证书后，安装器的 UAC 弹窗会显示 `Crossbow` 而非「未知发布者」。
仓库内 `docs/crossbow-win-cert.cer` 即签名用公开证书（私钥不在仓库）。

以管理员 PowerShell 执行：

```powershell
certutil -addstore -f Root crossbow-win-cert.cer
```

不需要时移除：

```powershell
certutil -delstore Root Crossbow
```

> 注意：信任根证书即信任该证书签发的一切内容。Crossbow 的签名私钥仅用于
> 本项目发布物；若你对此有顾虑，跳过此步完全不影响使用（仅 UAC 显示差异）。

## 常见问题

- **TUN 开关报「service not running」**：服务未注册（多见于绿色版/便携运行）。
  请重新运行完整安装器修复服务；便携目录不支持注册提权服务。
- **安装/升级后服务未启动**：以管理员执行 `sc start CrossbowService`。
- **校验安装包**：Release 页 SHA256SUMS 含全部资产哈希，
  `Get-FileHash Crossbow_*_x64-setup.exe` 比对即可。

## 服务边界与升级

- 管道拒绝远程客户端，双方验证对端来自同一安装目录；仅接受安装目录中的 `mihomo.exe`。服务停止时会退出所托管内核，重新启动时会清除已退出的进程状态。
- 读取用户配置、Provider 文件和打开日志时使用调用方身份；不以 SYSTEM 身份打开请求给出的日志路径。核心使用服务内的受保护运行目录，Provider 路径禁止绝对路径和 `..`，控制器仅监听 loopback 并要求密钥，禁止额外的控制管道和 UI 下载入口。
- 安装器会先等待旧服务停止，再覆盖文件；注册、启动失败会明确中止。卸载先停止服务并删除服务运行缓存。
- CI 验证编译、策略、安装打包；仍需 Windows 11 实机验收：安装/升级、TUN 独立联网、关闭后恢复普通代理、退出重开和卸载无残留。实机未验收前不应将 #81 记为全部完成。
