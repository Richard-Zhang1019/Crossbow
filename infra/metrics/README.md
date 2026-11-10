# Crossbow 下载量和 Star 快照

Worker 每天北京时间 00:05 读取公开的 GitHub Release 和仓库数据，在 D1 写入一条完整快照。范围包括正式版的 Windows 安装包、macOS DMG 和 Linux 安装归档。签名、更新索引、校验文件和独立更新归档不计入。Windows 更新器也会下载同一个安装包，GitHub 无法区分这部分请求。

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

重复触发本地定时事件，同一天仍只有一条记录。GitHub 读取使用公开 API，无需提供 Token。本地执行只写入本地 D1。

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

Worker 的定时触发器和数据库绑定均以 `wrangler.jsonc` 为准。若查询没有记录，检查 Worker 的 Cron 运行结果；GitHub 或 D1 请求失败时会保留已有快照，失败的日期不会写入半份数据。当前 HTTP 路由只提供健康检查。
