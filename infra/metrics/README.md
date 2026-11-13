# Crossbow 项目统计

Worker 每天北京时间 00:05 读取公开的 GitHub Release 和仓库数据，在 D1 写入一条完整快照。00:10 和 00:15 检查快照，缺失时才补采。00:17 后停止采集该日凌晨的快照。范围包括正式版的 Windows 安装包、macOS DMG 和 Linux 安装归档。签名、更新索引、校验文件和独立更新归档不计入。Windows 更新器也会下载同一个安装包，GitHub 无法区分这部分请求。

每条快照保存实际采集时间、Star 总数，以及按 GitHub 资产 ID 记录的下载累计数。首条快照是基线；计算趋势时按同一资产的相邻日快照取差。缺采的日期应显示缺失，跨多天差值不能当作单日数据。资产删除或替换时不把旧资产的累计数归零。

## 本地检查

在 `infra/metrics` 目录使用 Node.js 24：

```sh
node --test
npx --yes wrangler@4.160.0 d1 migrations apply DB --local
npx --yes wrangler@4.160.0 dev --test-scheduled
```

按 Wrangler 输出的本地端口调整下列命令，并在另一个终端运行：

```sh
curl -fsS http://127.0.0.1:8787/healthz
curl -fsS 'http://127.0.0.1:8787/cdn-cgi/local/scheduled?format=json'
npx --yes wrangler@4.160.0 d1 execute DB --local \
  --command 'SELECT day, stars, json_array_length(assets_json) AS assets FROM github_daily_snapshots'
```

补采窗口外触发本地事件会记录 `outside_window`。窗口内重复触发时，同一天仍只有一条记录。GitHub 读取使用公开 API，无需提供 Token。本地执行只写入本地 D1。单元测试使用可控时钟验证各采集场景。

## 首次部署

确认 Wrangler 登录的是创建 `crossbow-metrics` Worker 的账号，并在 `infra/metrics` 执行：

```sh
npx --yes wrangler@4.160.0 login
npx --yes wrangler@4.160.0 d1 migrations apply DB --remote
npx --yes wrangler@4.160.0 deploy
```

部署后访问 [Worker 健康检查](https://crossbow-metrics.aaaayu1019.workers.dev/healthz)，确认返回服务名称。首次成功采集后，在 D1 控制台运行：

```sql
SELECT day, collected_at, stars, json_array_length(assets_json) AS assets
FROM github_daily_snapshots
ORDER BY day DESC
LIMIT 7;
```

## 快照失败与补采

更新现有 Worker 时，先执行远程迁移 `0003_github_snapshot_cooldown.sql`，再部署 Worker。迁移创建一张独立表，用于保存 GitHub 限流的最早重试时间。

- GitHub 网络错误、超时、5xx 和已知的 D1 暂时性故障按 1 秒、2 秒间隔重试。采集和写入阶段分别计数。
- 遇到 GitHub 限流时，记录服务端给出的等待时间，后续触发先检查该时间。限流跨过当天补采窗口时保留缺口。
- 在 Cloudflare 的 `crossbow-metrics → Observability` 按快照日期和 `stage` 查找采集结果。日志的日期是北京时间采集日期，面板将这条快照归入前一个统计日。可用 `outcome`、`code` 和 `httpStatus` 判断失败阶段及原因。
- 00:17 后仍缺失的快照在面板显示「缺少快照」。后续累计值不能用于补写已过期的历史快照。

## 每日活跃设备

桌面端默认开启，可随时在「设置 → 应用与更新 → 匿名使用统计」关闭。按北京时间每天先尝试一次不使用显式代理的请求。当天尚未成功且 Crossbow 内核和系统代理或 TUN 已就绪时，改用本地混合端口补报。两条路径各有一次首发和最多五次重试，重试间隔依次为 1、2、5、15、30 分钟。代理关闭期间不消耗代理重试次数。当天成功后停止上报，重启应用也不会重复发送。

`POST /api/v1/active` 只接收 JSON 中的 `day`（北京时间当天对应的 Unix 日序号）、`daily_id`（当日生成的 32 位十六进制随机标识）、`platform`（`macos`、`windows`、`linux`）和 `version`（应用版本）。首次写入及重复请求均返回 HTTP 204。D1 每天清理过期去重记录，保留最近 7 天的记录和历史汇总。数据库不保存 IP、订阅、节点或访问记录。Worker 关闭自动请求日志和跟踪。持久日志只记录快照采集和日活清理任务的结果。指标统计的是开启统计且成功上报的活跃安装数。

请在分发带统计开关的客户端前先应用远程迁移并重新部署 Worker。部署后的汇总可在 D1 控制台查询：

```sql
SELECT date(day * 86400, 'unixepoch') AS beijing_day, SUM(devices) AS active_devices
FROM activity_daily_totals
GROUP BY day
ORDER BY day DESC
LIMIT 7;
```
