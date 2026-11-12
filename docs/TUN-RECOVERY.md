# Crossbow TUN 测试时的离线恢复

此恢复脚本独立于 Crossbow 应用运行。即使 TUN 测试使浏览器和 Codex 断网，仍可在本机终端执行。

## 每次测试前

1. 打开 Clash Verge，确认它的 **TUN 已关闭**，内核仍在运行，混合端口为 **7890**。可以关闭 Verge 的系统代理开关；不要退出 Verge。
2. 在终端运行预检：

   ```bash
   ./scripts/recover-tun-network.command --check
   ```

   看到“预检通过”后再开启 Crossbow TUN。预检只读取状态，不修改网络。

## 断网时恢复

在 Finder 中双击 [`scripts/recover-tun-network.command`](../scripts/recover-tun-network.command)，或在终端运行：

```bash
./scripts/recover-tun-network.command
```

按终端提示输入 macOS 管理员密码。脚本会确认 Verge 7890 正在监听、识别当前网络服务、停止 Crossbow 应用和仅属于 Crossbow 的 TUN 内核，等待其路由消失，再将该网络服务的 HTTP、HTTPS、SOCKS 代理指向 127.0.0.1:7890，并检查 GitHub 连通性。

脚本不会停止 Clash Verge 或其他 VPN，也不会盲目删除路由。若 Verge 未运行、TUN 路由仍存在或进程身份无法验证，它会停止并说明原因。恢复后 Crossbow 应用保持关闭；排查完成再打开。

Verge 使用其他混合端口时，可在终端先设置 `CROSSBOW_FALLBACK_PORT`，例如：

```bash
CROSSBOW_FALLBACK_PORT=7891 ./scripts/recover-tun-network.command --check
```

## 为排查保留断网现场

如果操作条件允许，先记下发生时间，再退出应用、切换代理或重启。下面几条命令只读，
可以帮助区分系统代理仍指向旧端口、TUN 捕获路由残留和物理网络没有默认出口：

```bash
date
/sbin/route -n get default
/sbin/route -n get 1.1.1.1
/usr/sbin/scutil --proxy
/usr/sbin/scutil --dns
```

分享结果前，隐去内部域名、PAC 地址等私人信息；不要上传订阅或内核配置。
Crossbow 的 `runtime/core-decision.log` 中，`core stop:` 和 `tun cleanup:`
可用于核对停止结果与代理恢复，`tun health:` 可用于核对是否发起自动恢复。
健康自检只在 macOS 本地 mihomo 的 gVisor TUN 会话运行。只有连续检测到同一目标
经混合端口可达、经 TUN 不可达时才请求正常重启；两条路径一起超时不会触发重启。
