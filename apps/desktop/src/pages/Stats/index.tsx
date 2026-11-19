import SegmentedControl from "@/components/SegmentedControl";
import { Button } from "@/components/Button";
import { notify } from "@/notifications/service";
import { useCallback, useEffect, useState } from "react";
import { fmtBytes } from "@/lib/format";
import {
  ipc,
  type TrafficStatRow,
  type TrafficStatsReport,
} from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { PageHead } from "@/pages/Home";

const RANGES = [
  { days: 1, labelKey: "stats.rangeToday" },
  { days: 7, labelKey: "stats.range7d" },
  { days: 30, labelKey: "stats.range30d" },
] as const;

const REFRESH_MS = 30_000;

/** 单行 TopN 条目：名称 + 占比条 + 上下行字节（条宽 = 总量/最大值）。 */
function TopRow({ row, max }: { row: TrafficStatRow; max: number }) {
  const total = row.up + row.down;
  const width = max > 0 ? Math.max((total / max) * 100, 2) : 0;
  return (
    <div className="cb-stat-row" title={`${row.key} · ↑${fmtBytes(row.up)} ↓${fmtBytes(row.down)}`}>
      <span className="cb-stat-key">{row.key}</span>
      <span className="cb-stat-bar-track">
        <span className="cb-stat-bar" style={{ width: `${width}%` }} />
      </span>
      <span className="cb-stat-bytes text-right">
        ↑{fmtBytes(row.up)} ↓{fmtBytes(row.down)}
      </span>
    </div>
  );
}

/** 每日总量堆叠柱状图（手写 SVG，不引图表库）：下行为主色、上行为次色。 */
function DailyChart({ daily }: { daily: TrafficStatRow[] }) {
  const max = Math.max(1, ...daily.map((d) => d.up + d.down));
  const slot = 100 / Math.max(daily.length, 1);
  return (
    <svg
      viewBox="0 0 100 44"
      preserveAspectRatio="none"
      className="cb-stat-chart"
      aria-hidden="true"
    >
      {daily.map((d, i) => {
        const x = i * slot + slot * 0.15;
        const w = slot * 0.7;
        const downH = (d.down / max) * 38;
        const upH = (d.up / max) * 38;
        return (
          <g key={d.key}>
            <title>{`${d.key} · ↑${fmtBytes(d.up)} ↓${fmtBytes(d.down)}`}</title>
            <rect x={x} y={42 - downH} width={w} height={downH} fill="var(--cb-down)" />
            <rect x={x} y={42 - downH - upH} width={w} height={upH} fill="var(--cb-up)" />
          </g>
        );
      })}
    </svg>
  );
}

/** #567 统计页：按域名/出口节点的本地累计 TopN + 每日总量（纯本地，不上云）。 */
export default function StatsPage() {
  const t = useT();
  const [days, setDays] = useState<number>(7);
  const [report, setReport] = useState<TrafficStatsReport | null>(null);
  const [failed, setFailed] = useState(false);

  const load = useCallback(
    async (silent = false) => {
      try {
        setReport(await ipc.trafficStats(days));
        setFailed(false);
      } catch (error) {
        // 读失败保留上次数据（真实状态），页面提示 + 重试（#562 同模式）
        setFailed(true);
        if (!silent) notify.error(error, "stats-load");
      }
    },
    [days],
  );

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(true), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const daily = report?.daily ?? [];
  const isEmpty = !report || (report.domains.length === 0 && report.nodes.length === 0);

  return (
    <div className="cb-stats-page">
      <PageHead title={t("stats.title")} meta={t("stats.hint")} />
      <div className="cb-stat-toolbar">
        <SegmentedControl
          label={t("stats.range")}
          value={String(days)}
          onChange={(value) => {
            if (value) setDays(Number(value));
          }}
          options={RANGES.map((r) => ({ value: String(r.days), label: t(r.labelKey) }))}
        />
        {failed && (
          <Button size="small" onClick={() => void load()}>
            {t("stats.retry")}
          </Button>
        )}
      </div>
      {isEmpty ? (
        failed ? (
          <div className="cb-empty">
            <p role="status">{t("stats.loadFailed")}</p>
            <Button size="small" onClick={() => void load()}>
              {t("stats.retry")}
            </Button>
          </div>
        ) : (
          <div className="cb-empty">
            <p>{t("stats.empty")}</p>
          </div>
        )
      ) : (
        <>
          {daily.length > 1 && (
            <section className="cb-card cb-stat-section">
              <h3>{t("stats.daily")}</h3>
              <DailyChart daily={daily} />
            </section>
          )}
          <div className="cb-stats-grid">
            <section className="cb-card cb-stat-section">
              <h3>{t("stats.topDomains")}</h3>
              {report!.domains.length === 0 ? (
                <p className="cb-stat-none">{t("stats.noData")}</p>
              ) : (
                report!.domains.map((row) => (
                  <TopRow key={row.key} row={row} max={Math.max(1, ...report!.domains.map((r) => r.up + r.down))} />
                ))
              )}
            </section>
            <section className="cb-card cb-stat-section">
              <h3>{t("stats.topNodes")}</h3>
              {report!.nodes.length === 0 ? (
                <p className="cb-stat-none">{t("stats.noData")}</p>
              ) : (
                report!.nodes.map((row) => (
                  <TopRow key={row.key} row={row} max={Math.max(1, ...report!.nodes.map((r) => r.up + r.down))} />
                ))
              )}
            </section>
          </div>
        </>
      )}
    </div>
  );
}
