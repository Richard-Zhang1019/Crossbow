import { useClipboardFeedback } from "@/hooks/useClipboardFeedback";
import {Icon} from "@/components/Icon";
import Select from "@/components/Select";
import {SearchField} from "@/components/Controls";
import { ButtonBase, Button } from "@/components/Button";
import NodeName from "@/components/NodeName";
import { notify } from "@/notifications/service";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ipc, type LogEntry } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import "./Logs.css";
import { LogHistory } from "./logHistory";
import { LogReadingWindow } from "./logReading";

const MAX_LINES = 2000;
const RENDER_TAIL = 500;

const LEVELS = ["debug", "info", "warning", "error"] as const;
type Level = (typeof LEVELS)[number];

const LEVEL_COLOR: Record<string, string> = {
  debug: "var(--cb-faint)",
  info: "var(--cb-text)",
  warning: "var(--cb-warn)",
  error: "var(--cb-bad)",
};

const MAX_PENDING = 400;

/** sing-box（clash_api）同连接多行的中间态（以行首连接 id 关联）。 */
interface SbPending {
  type: string;
  process: string;
  source: string;
  target: string;
  outbound: string;
  /** 首次出现时刻（ms），超时强制成行用 */
  seen: number;
}

/** sing-box 日志消费：把 inbound/router/outbound 同连接多行聚合为一条
 *  mihomo 风格合成行（复用现有结构化渲染）；非 sing-box 格式或无法
 *  识别的组件行返回 false 走原始兜底。 */
function consumeSingBox(
  payload: string,
  pending: Map<string, SbPending>,
  out: LogEntry[],
): boolean {
  const m = /^\[(\d+)\s+\d+ms\]\s+([^:]+):\s+(.*)$/.exec(payload);
  if (!m) return false;
  const [, id, comp, msg] = m;
  const tagOf = (c: string) => {
    const s = c.indexOf("[");
    return s >= 0 ? c.slice(s + 1, c.lastIndexOf("]")) : "";
  };
  const entry = (): SbPending => {
    let e = pending.get(id);
    if (!e) {
      e = {
        type: "",
        process: "",
        source: "",
        target: "",
        outbound: "",
        seen: Date.now(),
      };
      pending.set(id, e);
    }
    return e;
  };
  if (comp.startsWith("inbound/")) {
    const tag = tagOf(comp); // tun-in / mixed-in
    const e = entry();
    e.type =
      tag === "tun-in"
        ? "TUN"
        : tag === "mixed-in"
          ? "MIXED"
          : tag.toUpperCase();
    const TO = "inbound connection to ";
    const FROM = "inbound connection from ";
    if (msg.startsWith(TO)) e.target = msg.slice(TO.length);
    if (msg.startsWith(FROM)) e.source = msg.slice(FROM.length);
    return true;
  }
  if (comp === "router" && msg.startsWith("found process path: ")) {
    const rest = msg.slice("found process path: ".length);
    const path = rest.split(", user:")[0]?.trim() ?? "";
    entry().process = path.split("/").pop() ?? path;
    return true;
  }
  if (
    comp.startsWith("outbound/") &&
    msg.startsWith("outbound connection to ")
  ) {
    const e = entry();
    e.outbound = tagOf(comp) || comp;
    e.target = msg.slice("outbound connection to ".length);
    const left = e.source ? `${e.source}(${e.process})` : `(${e.process})`;
    out.push({
      type: "info",
      payload: `[${e.type || "TCP"}] ${left} --> ${e.target} match Match using ${e.outbound}`,
    });
    pending.delete(id);
    return true;
  }
  return false;
}

/** mihomo 连接日志的结构化视图；解析失败的行（warning/error 等）走原始整行。 */
interface ParsedLog {
  network: string;
  process: string;
  source: string;
  target: string;
  rule: string;
  outbound: string;
}

function parseLog(payload: string): ParsedLog | null {
  const m = /^\[(TCP|UDP)\]\s+(.+?)\s+-->\s+(.+)$/.exec(payload);
  if (!m) return null;
  const [, network, left, right] = m;
  // 左段：ip:port(进程名) —— 进程名内可能含括号，取最后一个括号组
  const pm = /^(.*?)\((.*)\)$/.exec(left);
  const source = pm ? pm[1] : left;
  const process = pm ? pm[2] : "";
  // 右段：目标 match 规则 using 出口
  const rm = /^(.+?)\s+match\s+(.*?)\s+using\s+(.+)$/.exec(right);
  if (!rm) return null;
  return {
    network,
    process,
    source,
    target: rm[1],
    rule: rm[2],
    outbound: rm[3],
  };
}

/** 规则段拆成「类型 + 明细」：DomainSuffix(qq.com) → DomainSuffix / qq.com */
function splitRule(rule: string): { type: string; detail: string } {
  const m = /^([A-Za-z]+)\((.*)\)$/.exec(rule);
  return m ? { type: m[1], detail: m[2] } : { type: rule, detail: "" };
}

const isDirect = (outbound: string) => /^direct$/i.test(outbound.trim());

/** 日志页：结构化表格（表头 + 类型/进程/目标/规则/出口），级别过滤 + 关键字过滤。 */
export default function LogsPage() {
  const t = useT();
  const [lines, setLines] = useState<LogEntry[]>([]);
  const [enabled, setEnabled] = useState<Record<Level, boolean>>({
    debug: false,
    info: true,
    warning: true,
    error: true,
  });
  const [keyword, setKeyword] = useState("");
  const [connected, setConnected] = useState(false);
  const [coreRunning, setCoreRunning] = useState(false);
  const [loading, setLoading] = useState(true);
  const [clearing, setClearing] = useState(false);
  const historyRef = useRef(new LogHistory());
  const pageActive = useRef(false);
  const statusGeneration = useRef(0);
  const pinned = useRef(true);
  const [following,setFollowing]=useState(true);
  const [exporting,setExporting]=useState(false);
  const exportFlight=useRef(false);
  const { copied: copiedLine, copy } = useClipboardFeedback<LogEntry>("logs-copy");
  // Keep the reading target still while new logs continue to arrive in the buffer.
  const [readingLines, setReadingLines] = useState<LogEntry[] | null>(null);
  const reading = useRef(new LogReadingWindow<LogEntry>());
  const latestLines = useRef(lines);
  latestLines.current = lines;
  // 读窗锚点：冻结开始时视口顶部的行元素与其在盒内的偏移。释放时把同一
  // DOM 节点调回原屏幕位置——冻结期间新到的行就不会把可见内容整体顶走
  // （hover 抖动）。贴底进入的（atBottom）释放时直接追新底部，保持原
  // 跟随体验；锚点行已被裁出渲染尾窗时同样退回跟随。
  const readingAnchor = useRef<
    { el: Element; offset: number; atBottom: boolean } | null
  >(null);
  const readingHadWindow = useRef(false);
  const holdReading = (kind: "pointer" | "focus", hold: boolean) => {
    const box = boxRef.current;
    if (hold && box) {
      const boxRect = box.getBoundingClientRect();
      const rows = box.querySelectorAll(".cb-log-row, .cb-log-raw");
      let top: Element | null = null;
      for (const row of rows) {
        const r = row.getBoundingClientRect();
        if (r.bottom <= boxRect.top + 1) continue;
        top = row;
        break;
      }
      readingAnchor.current =
        top !== null
          ? {
              el: top,
              offset: top.getBoundingClientRect().top - boxRect.top,
              atBottom:
                box.scrollHeight - box.scrollTop - box.clientHeight < 2,
            }
          : null;
    }
    const next = reading.current.hold(kind, hold, latestLines.current);
    // 没有可释放的窗口（其他 hold 仍在或本就无窗）时丢弃锚点；
    // 真正进入冻结时保留，由释放路径消费。
    if (next === null && readingLines === null) readingAnchor.current = null;
    setReadingLines(next);
  };
  const boxRef = useRef<HTMLDivElement>(null);
  const subscriptionGeneration = useRef(0);
  // sing-box 同连接多行聚合的中间态（跨批次保留，#57）
  const pendingRef = useRef(new Map<string, SbPending>());

  const consumeEntries = useCallback((entries: LogEntry[]) => {
    const out: LogEntry[] = [];
    for (const e of entries) {
      // sing-box（clash_api）日志：同连接多行聚合后再进表（#57）
      if (!consumeSingBox(e.payload, pendingRef.current, out)) out.push(e);
    }
    // 超时成行：挂起超过 3s 的连接（outbound 先到/日志乱序等场景）
    // 以已有部分信息强制成行，避免静默丢失；无目标也无出口的行
    // （孤立 router 行等）不含连接信息，直接丢弃
    const now = Date.now();
    for (const [id, e] of [...pendingRef.current]) {
      if (now - e.seen <= 3000) continue;
      pendingRef.current.delete(id);
      if (!e.target && !e.outbound) continue;
      const left = e.source ? `${e.source}(${e.process})` : `(${e.process})`;
      out.push({
        type: "info",
        payload: `[${e.type || "TCP"}] ${left} --> ${e.target || "?"} match Match using ${
          e.outbound || "?"
        }`,
      });
    }
    // 防御：未凑齐 outbound 的连接积压过多时丢弃最旧的中间态
    if (pendingRef.current.size > MAX_PENDING) {
      const drop = pendingRef.current.size - MAX_PENDING;
      for (const k of [...pendingRef.current.keys()].slice(0, drop)) {
        pendingRef.current.delete(k);
      }
    }
    setLines((prev) => {
      const next = [...prev, ...out];
      return next.length > MAX_LINES
        ? next.slice(next.length - MAX_LINES)
        : next;
    });
  }, []);

  useEffect(() => {
    const generation = ++subscriptionGeneration.current;
    let active = true;
    pageActive.current = true;
    const subscriber = crypto.randomUUID();
    const history = new LogHistory();
    historyRef.current = history;
    setLoading(true);
    const unstatus = ipc.onLogsStatus((status) => {
      if (active && status.generation >= statusGeneration.current) {
        statusGeneration.current = status.generation;
        setConnected(status.connected);
      }
    });
    let receivedCoreEvent = false;
    const uncore = ipc.onCoreStatus((status) => {
      if (!active) return;
      receivedCoreEvent = true;
      setCoreRunning(status.state === "Running");
      if (status.state !== "Running") setConnected(false);
    });
    void uncore
      .then(() => ipc.coreStatus())
      .then((status) => {
        if (active && !receivedCoreEvent)
          setCoreRunning(status.state === "Running");
      })
      .catch(() => {});
    // Listen first, then join the cache snapshot and in-flight events by sequence.
    const unlisten = ipc.onLogs((entries) => {
      if (active) consumeEntries(history.accept(entries));
    });

    // StrictMode runs setup → cleanup → setup in development. Waiting for the
    // listener avoids losing the first batch; the generation guard prevents a
    // stale setup from subscribing after the page has already been replaced.
    const subscribe = Promise.all([unlisten, unstatus, uncore]).then(() => {
      if (!active || subscriptionGeneration.current !== generation) return;
      return ipc.subscribeLogs(subscriber).then((snapshot) => {
        if (!active) return;
        pendingRef.current.clear();
        setLines([]);
        consumeEntries(history.hydrate(snapshot));
        if (snapshot.generation >= statusGeneration.current) {
          statusGeneration.current = snapshot.generation;
          setConnected(snapshot.connected);
        }
        setLoading(false);
      });
    }).catch((error) => {
      if (!active) return;
      consumeEntries(history.resume());
      setLoading(false);
      notify.error(error, "logs-stream");
    });

    return () => {
      active = false;
      pageActive.current = false;
      setConnected(false);
      for (const pending of [unlisten, unstatus, uncore])
        void pending.then((remove) => remove());
      // Delay unsubscription until any in-flight subscribe has settled. A
      // StrictMode remount advances the generation synchronously and keeps the
      // replacement subscription alive.
      void subscribe
        .catch(() => {})
        .finally(() => {
          queueMicrotask(() => {
            if (subscriptionGeneration.current === generation) {
              ipc.unsubscribeLogs(subscriber).catch(() => {});
            }
          });
        });
    };
  }, [consumeEntries]);

  const clear = async () => {
    if (clearing || loading) return;
    const history = historyRef.current;
    history.pause();
    setClearing(true);
    const result = await notify.run("logs-clear", t("logs.clear"), () => ipc.clearLogs(), undefined,
      { feedback: "error-only", titleKey: "logs.clear" });
    if (!pageActive.current || historyRef.current !== history) return;
    if (result.ok) {
      pendingRef.current.clear();
      setLines([]);
      setReadingLines(reading.current.clear());
      consumeEntries(history.hydrate(result.value));
    } else {
      consumeEntries(history.resume());
    }
    setClearing(false);
  };

  // Follow before paint; a post-paint scroll flashes the previous position on each batch.
  useLayoutEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const hadReading = readingHadWindow.current;
    readingHadWindow.current = readingLines !== null;
    if (readingLines === null && hadReading) {
      // 释放读窗：DOM 已换回活数据（绘制前）。中列阅读者把锚点行调回
      // 冻结时所在的屏幕位置，视口内容像素级静止；贴底进入的（atBottom）
      // 或锚点行已被裁掉的直接追新底部，保持原跟随体验。
      const anchor = readingAnchor.current;
      readingAnchor.current = null;
      if (anchor && !anchor.atBottom && anchor.el.isConnected) {
        const boxRect = box.getBoundingClientRect();
        const shift =
          anchor.el.getBoundingClientRect().top - boxRect.top - anchor.offset;
        if (Math.abs(shift) > 0.5) box.scrollTop += shift;
        return;
      }
    }
    if (pinned.current && readingLines === null) {
      box.scrollTop = box.scrollHeight;
    }
  }, [lines, readingLines]);

  const filtered = useMemo(() => {
    const kw = keyword.trim().toLowerCase();
    return (readingLines ?? lines)
      .filter((l) => enabled[(l.type as Level) ?? "info"] ?? true)
      .filter((l) => !kw || l.payload.toLowerCase().includes(kw));
  }, [lines, readingLines, enabled, keyword]);

  // #431：解析随 filtered 记忆化——hover/复制态等无关重渲染不再逐行重跑正则
  const shown = useMemo(
    () =>
      filtered.slice(-RENDER_TAIL).map((entry) => ({
        entry,
        parsed: parseLog(entry.payload),
      })),
    [filtered],
  );
  const copyLine = (line: LogEntry) =>
    copy(line, () => ipc.redactLogLines([`[${line.type}] ${line.payload}`]));
  const exportLogs=async()=>{if(exportFlight.current)return;exportFlight.current=true;setExporting(true);try{const saved=await ipc.exportLogLines(filtered.map(l=>`[${l.type}] ${l.payload}`));if(saved)notify.success(t("logs.exported"),"logs-export");}catch(e){notify.error(e,"logs-export");}finally{exportFlight.current=false;setExporting(false);}};
  const resume=()=>{pinned.current=true;setFollowing(true);if(boxRef.current)boxRef.current.scrollTop=boxRef.current.scrollHeight;};
  const copyButton=(line:LogEntry)=><ButtonBase className="cb-log-copy" onClick={()=>void copyLine(line)} aria-label={t("logs.copyLine")} title={t("logs.copyLine")}>{copiedLine===line?"✓":t("workflow.copy")}</ButtonBase>;

  return (
    <div className="cb-logs-page">
      <div className="cb-log-toolbar cb-log-single-toolbar">
        <h1 className="cb-log-title">{t("logs.title")}</h1>
        <Select
          multiple
          size="compact"
          className="cb-log-level-select"
          label={t("logs.levels")}
          selectionLabel={t("logs.selectedLevels", { n: LEVELS.filter(level => enabled[level]).length })}
          value={LEVELS.filter(level => enabled[level])}
          options={LEVELS.map(level => ({ value: level, label: level }))}
          onChange={selected => setEnabled({
            debug: selected.includes("debug"), info: selected.includes("info"),
            warning: selected.includes("warning"), error: selected.includes("error"),
          })}
        />
        <SearchField className="cb-log-search" id="cb-log-search" label={t("logs.keyword")} value={keyword} onChange={e=>setKeyword(e.target.value)} placeholder={t("logs.keyword")}/>
        <div className="cb-data-actions">
          <Button title={!following?t("logs.followPaused"):undefined} onClick={()=>{if(following){pinned.current=false;setFollowing(false)}else resume()}}>{t(following?"logs.pauseFollow":"logs.follow")}</Button>
          {!following&&<Button className="cb-log-bottom" iconOnly title={t("logs.bottom")} aria-label={t("logs.bottom")} onClick={resume}><Icon name="download"/></Button>}
          <Button className="cb-log-export cb-log-secondary-action" aria-label={t("logs.export")} disabled={!filtered.length||exporting} loading={exporting} title={t("logs.exportHint",{n:filtered.length})} onClick={()=>void exportLogs()}><Icon name="download"/><span className="cb-log-action-label">{t("logs.export")}</span></Button>
          <Button className="cb-log-clear cb-log-secondary-action" aria-label={t("logs.clear")} title={t("logs.clear")} onClick={clear} disabled={lines.length===0||loading} loading={clearing}><Icon name="trash"/><span className="cb-log-action-label">{t("logs.clear")}</span></Button>
        </div>
        <span className="sr-only" role="status">{!following?t("logs.followPaused"):""}</span>
      </div>
      <div
        ref={boxRef}
        onPointerEnter={event => { if (event.pointerType === "mouse") holdReading("pointer", true); }}
        onPointerLeave={() => holdReading("pointer", false)}
        onFocusCapture={() => holdReading("focus", true)}
        onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) holdReading("focus", false); }}
        onScroll={(e) => {
          const el = e.currentTarget;
          const atBottom=el.scrollHeight-el.scrollTop-el.clientHeight<40;
          if(!atBottom){pinned.current=false;setFollowing(false);}
        }}
        className="cb-card cb-log-scroll cb-selectable cb-mono text-[13px] leading-5"
      >
        {shown.length === 0 ? (
          <div
            className="flex h-full flex-col items-center justify-center gap-3"
            style={{ color: "var(--cb-text-dim)" }}
          >
            <span>
              {loading
                ? t("logs.loading")
                : lines.length > 0
                  ? t("logs.filteredEmpty")
                  : !coreRunning
                    ? t("logs.stopped")
                    : connected
                      ? t("logs.empty")
                      : t("logs.connecting")}
            </span>
            {!loading && lines.length === 0 && (!coreRunning || !connected) && (
              <Button
                onClick={() =>
                  window.dispatchEvent(
                    new CustomEvent("cb:goto", { detail: "diagnostics" }),
                  )
                }
              >
                {t("nav.diagnostics")}
              </Button>
            )}
          </div>
        ) : (
          <>
            <div className="cb-log-grid cb-log-head">
              <span>{t("logs.col.type")}</span>
              <span>{t("logs.col.process")}</span>
              <span>{t("logs.col.target")}</span>
              <span>{t("logs.col.rule")}</span>
              <span>{t("logs.col.outbound")}</span>
            </div>
            {shown.map(({ entry: l, parsed }) => {
              if (!parsed) {
                return (
                  <div
                    key={reading.current.key(l)}
                    className="cb-log-raw"
                    style={{
                      color: LEVEL_COLOR[l.type] ?? "var(--cb-text-dim)",
                    }}
                  >
                    {l.payload}
                    {copyButton(l)}
                  </div>
                );
              }
              const rule = splitRule(parsed.rule);
              const direct = isDirect(parsed.outbound);
              return (
                <div key={reading.current.key(l)} className="cb-log-grid cb-log-row">
                  <span>
                    <span
                      className="cb-log-tag"
                      style={
                        parsed.network === "UDP"
                          ? {
                              color: "var(--cb-down)",
                              background: "rgba(183, 166, 248, 0.13)",
                            }
                          : {
                              color: "var(--cb-accent)",
                              background: "var(--cb-accent-soft)",
                            }
                      }
                    >
                      {parsed.network}
                    </span>
                  </span>
                  <span className="cb-log-process">
                    <span className="truncate">{parsed.process || "—"}</span>
                    <span className="cb-log-source">
                      {parsed.source || "—"}
                    </span>
                  </span>
                  <span
                    className="truncate"
                    style={{ color: "var(--cb-text)" }}
                  >
                    {parsed.target}
                  </span>
                  <span
                    className="truncate"
                    style={{ color: "var(--cb-text-dim)" }}
                  >
                    {rule.type}
                    {rule.detail && (
                      <span style={{ color: "var(--cb-faint)" }}>
                        {" "}
                        · {rule.detail}
                      </span>
                    )}
                  </span>
                  <span>
                    <span
                      className="cb-log-outbound"
                      style={
                        direct
                          ? {
                              color: "var(--cb-ok)",
                              background: "rgba(74, 222, 128, 0.1)",
                            }
                          : {
                              color: "var(--cb-accent)",
                              background: "var(--cb-accent-soft)",
                            }
                      }
                    >
                      <NodeName name={parsed.outbound} />
                    </span>
                  </span>
                  {copyButton(l)}
                </div>
              );
            })}
          </>
        )}
      </div>
    </div>
  );
}
