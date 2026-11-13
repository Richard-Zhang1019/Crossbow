# Crossbow 首次连接

关联 #467 · v01 · 已实现，待原生验收

在概览状态标题下放一条紧凑的步骤卡。卡片使用现有主题表面和小号按钮，随内容换行。导入复用配置页弹窗，下载和启动完成后根据内核状态继续。下载失败在同一位置提供重试，端口冲突定位到网络设置。接管步骤将键盘焦点移到现有开关，开启和授权由用户操作。

验证使用 GitHub 的现有站点测试，通过 Crossbow 混合端口发出请求。连接状态卡分别显示内核进程、接管配置和本次 GitHub 结果。验证成功后收起引导。切页和重开时根据保存的引导进度和当前状态恢复步骤。

## 源稿和预览

源稿是生产组件及本目录的模拟入口。预览数据只存入浏览器会话，不调用原生网络或配置命令。在仓库根目录运行以下命令后打开 [本地预览](http://127.0.0.1:5245/)。

```sh
pnpm --filter @crossbow/desktop exec vite --config ../../docs/designs/2026-11-13-first-connection/v01/vite.config.mts
```

- [浅色导入](previews/import-light.jpg) · [浅色验证](previews/verify-light.jpg) · [880 × 560 深色接管](previews/takeover-dark.jpg)
- `?missing&busyPort&cancelTun&probeFail` 模拟首次下载失败、默认端口占用、首次 TUN 授权取消和首次验证失败。
- `?en&dark` 模拟随包内核和深色英文界面。`?configured` 模拟已经接管的配置。`?stopped&missing` 用于检查节点和诊断的引导入口。

浏览器模拟覆盖首次操作、重开、切页和错误恢复。实际下载、系统代理、TUN 授权和打包后的 macOS、Windows 初装仍需原生验收。
