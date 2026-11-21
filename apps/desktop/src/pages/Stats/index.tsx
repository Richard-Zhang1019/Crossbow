import SegmentedControl from "@/components/SegmentedControl";
import { Button, ButtonBase } from "@/components/Button";
import { Icon } from "@/components/Icon";
import NodeName from "@/components/NodeName";
import { notify } from "@/notifications/service";
import { useCallback, useEffect, useRef, useState } from "react";
import { fmtBytes } from "@/lib/format";
import { ipc, type TrafficStatRow, type TrafficStatsReport } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import './Stats.css';

const RANGES = [{ days: 1, labelKey: "stats.rangeToday" }, { days: 7, labelKey: "stats.range7d" }, { days: 30, labelKey: "stats.range30d" }] as const;
const sum = (rows: TrafficStatRow[]) => rows.reduce((a, r) => ({ up: a.up + r.up, down: a.down + r.down }), { up: 0, down: 0 });

function Ranking({ rows, total, nodes = false }: { rows: TrafficStatRow[]; total: number; nodes?: boolean }) {
  const t = useT();
  return <section className="cb-stats-ranking">
    <header><h2>{t(nodes ? 'stats.topNodes' : 'stats.topDomains')}</h2><span>{t('stats.share')}</span></header>
    <div className="cb-stats-ranking-list">
      {!rows.length ? <p className="cb-stats-empty-list">{t('stats.noData')}</p> : rows.map(row => {
        const bytes = row.up + row.down;
        const percent = total > 0 ? bytes / total * 100 : 0;
        return <div className="cb-stats-entry" key={row.key}>
          <div className="cb-stats-entry-title"><span title={row.key}>{row.key === "__other__" ? t('stats.otherBucket') : nodes ? <NodeName name={row.key} /> : row.key}</span><strong>{fmtBytes(bytes)}</strong></div>
          <div className="cb-stats-entry-meta"><span>↑ {fmtBytes(row.up)}<span className="cb-stats-meta-down">↓ {fmtBytes(row.down)}</span></span><span>{percent.toFixed(1)}%</span></div>
          <div className="cb-stats-track" aria-hidden="true"><span style={{width:`${Math.min(percent,100)}%`}} /></div>
        </div>;
      })}
    </div>
  </section>;
}

export default function StatsPage() {
  const t = useT();
  const [days, setDays] = useState(7);
  const [paused, setPaused] = useState<boolean | null>(null);
  const [controlPending, setControlPending] = useState(false);
  const controlBusy = useRef(false);
  const controlRequest = useRef(0);
  const [clearing, setClearing] = useState(false);
  const [result, setResult] = useState<{ days: number; report: TrafficStatsReport } | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const request = useRef(0);
  const load = useCallback(async (silent = false) => {
    if (controlBusy.current) return;
    const id = ++request.current;
    if (!silent) { setLoading(true); setFailed(false); }
    try {
      const report = await ipc.trafficStats(days);
      if (request.current !== id) return;
      setResult({ days, report }); setFailed(false);
      void notify.resolve('stats-load');
    } catch (error) {
      if (request.current !== id) return;
      setFailed(true);
      if (!silent) notify.error(error,'stats-load');
    } finally {
      if (request.current === id) setLoading(false);
    }
  }, [days]);
  const loadControl = useCallback(async () => {
    const id = ++controlRequest.current;
    try {
      const control = await ipc.trafficStatsControl();
      if (controlRequest.current !== id) return;
      setPaused(control.paused);
      if (control.warning) notify.error(control.warning, 'stats-control');
      else void notify.resolve('stats-control');
    } catch (error) {
      if (controlRequest.current !== id) return;
      setPaused(null); notify.error(error, 'stats-control');
    }
  }, []);
  useEffect(() => {
    void loadControl();
    return () => { controlRequest.current++; };
  }, [loadControl]);
  const changeControl = async (clear: boolean) => {
    if (controlBusy.current || (!clear && paused === null)) return;
    controlBusy.current = true;
    controlRequest.current++;
    setControlPending(true);
    setClearing(clear);
    request.current++;
    try {
      if (clear) {
        await ipc.clearTrafficStats();
        setSelected(null);
        setResult(null);
        await loadControl();
        notify.info(t('stats.cleared'), 'stats-clear');
      } else {
        await ipc.setTrafficStatsPaused(!paused);
        setPaused(!paused);
        notify.info(paused ? t('stats.resumed') : t('stats.pausedNotice'), 'stats-control');
      }
    } catch (error) {
      notify.error(error, clear ? 'stats-clear' : 'stats-control');
      await loadControl();
    } finally {
      controlBusy.current = false;
      setControlPending(false);
      setClearing(false);
      void load(true);
    }
  };
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(true), 30_000);
    return () => { ++request.current; window.clearInterval(timer); };
  }, [load]);

  // A previous range must never be shown beneath the new range's label.
  const report = result?.days === days ? result.report : null;
  const daily = report?.daily ?? [];
  const totals = sum(daily);
  const total = totals.up + totals.down;
  const max = Math.max(1, ...daily.map(r => r.up + r.down));
  const today = new Date();
  const chartDaily = Array.from({ length: days }, (_, index) => {
    const date = new Date(today.getFullYear(), today.getMonth(), today.getDate() - days + index + 1);
    const key = `${date.getFullYear()}-${String(date.getMonth()+1).padStart(2,'0')}-${String(date.getDate()).padStart(2,'0')}`;
    const row = daily.find(r => r.key === key);
    return { key, up: row?.up ?? 0, down: row?.down ?? 0, missing: !row };
  });
  const activeDay = chartDaily.find(r => r.key === selected) ?? chartDaily.at(-1);
  const range = RANGES.find(r => r.days === days)!;
  return <div className="cb-stats-page" aria-busy={loading}>
    <header className="cb-stats-heading"><div><h1>{t('stats.title')}</h1><p>{t('stats.localHint')}</p></div>
      <div className="cb-stats-controls"><SegmentedControl label={t('stats.range')} value={String(days)} onChange={v => {if(v){setDays(Number(v));setSelected(null);}}} options={RANGES.map(r => ({value:String(r.days),label:t(r.labelKey)}))}/><Button iconOnly aria-label={t('stats.refresh')} title={t('stats.refresh')} onClick={() => { void load(); void loadControl(); }} disabled={controlPending} loading={loading}><Icon name="refresh" size={16}/></Button>
        <div className="cb-stats-privacy">
          <Button size="small" disabled={paused === null || controlPending} loading={controlPending && !clearing} onClick={() => void changeControl(false)}>{paused ? t('stats.resumeRecord') : t('stats.pauseRecord')}</Button>
          <Button size="small" tone="danger" disabled={controlPending} loading={clearing} onClick={() => void changeControl(true)}>{t('stats.clearLocal')}</Button>
        </div>
      </div>
    </header>
    {paused && <p className="cb-stats-paused-note" role="status">{t('stats.pausedActive')}</p>}
    {failed && <div className="cb-stats-read-state" role="status"><span>{t('stats.loadFailed')}{report ? ` · ${t('stats.previous')}` : ''}</span><Button size="small" onClick={() => void load()}>{t('stats.retry')}</Button></div>}
    {!report ? <div className="cb-stats-empty" role="status">{t(loading ? 'stats.loading' : failed ? 'stats.loadFailed' : 'stats.empty')}</div> : total === 0 ? <div className="cb-stats-empty" role="status"><Icon name="activity" size={24}/><h2>{t('stats.emptyTitle')}</h2><p>{t('stats.empty')}</p></div> : <>
      <section className="cb-stats-overview">
        <div className="cb-stats-total"><p>{t(range.labelKey)} · {t('stats.observed')}</p><strong>{fmtBytes(total)}</strong><dl><div><dt><i className="cb-stats-dot down"/>{t('stats.download')}</dt><dd>{fmtBytes(totals.down)}</dd></div><div><dt><i className="cb-stats-dot up"/>{t('stats.upload')}</dt><dd>{fmtBytes(totals.up)}</dd></div></dl></div>
        <div className="cb-stats-trend"><header><h2>{t('stats.daily')}</h2><span className="cb-stats-day-detail">{activeDay?.key} <b>{activeDay?.missing ? t('stats.noData') : fmtBytes((activeDay?.up ?? 0) + (activeDay?.down ?? 0))}</b></span></header>
          <div className="cb-stats-chart-body"><div className="cb-stats-scale"><span>{fmtBytes(max)}</span><span>{fmtBytes(max/2)}</span><span>0 B</span></div><div className="cb-stats-plot" style={{gridTemplateColumns:`repeat(${Math.max(chartDaily.length,1)},minmax(0,1fr))`}}>
            {chartDaily.map(d => <ButtonBase className="cb-stats-day" key={d.key} aria-label={d.missing ? `${d.key} · ${t('stats.noData')}` : `${d.key} · ↓ ${fmtBytes(d.down)} · ↑ ${fmtBytes(d.up)}`} aria-pressed={activeDay?.key === d.key} onClick={() => setSelected(d.key)} title={d.missing ? `${d.key} · ${t('stats.noData')}` : `${d.key} · ↓ ${fmtBytes(d.down)} · ↑ ${fmtBytes(d.up)}`}><span className="cb-stats-stack" style={{height:`${(d.up+d.down)/max*100}%`}}><span className="cb-stats-stack-up" style={{flexGrow:d.up}}/><span className="cb-stats-stack-down" style={{flexGrow:d.down}}/></span></ButtonBase>)}
          </div></div><div className="cb-stats-dates"><span>{chartDaily[0]?.key.slice(5).replace('-','/')}</span><span>{chartDaily.at(-1)?.key.slice(5).replace('-','/')}</span></div>
        </div>
      </section>
      <div className="cb-stats-rankings"><Ranking rows={report.domains} total={total}/><Ranking rows={report.nodes} total={total} nodes/></div>
      <p className="cb-stats-footnote">{t('stats.sampleNote')}</p>
    </>}
  </div>;
}
