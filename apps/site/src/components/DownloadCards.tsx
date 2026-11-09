import { useCallback, useEffect, useState } from "react";
import { createI18n, type Lang } from "../i18n";
import { LATEST } from "../site-meta";

export interface ReleaseInfo {
  version: string;
  dmgUrl: string | null;
  windowsUrl: string | null;
  linuxUrl: string | null;
  checksumsUrl: string | null;
  date: string | null;
}

export type ReleaseStatus = "loading" | "ready" | "failed";

const API = "https://api.github.com/repos/Richard-Zhang1019/Crossbow/releases/latest";
const CACHE_KEY = "crossbow-release-cache-v1";

interface CacheShape {
  savedAt: string;
  info: ReleaseInfo;
}

function readCache(): ReleaseInfo | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CacheShape;
    return parsed?.info?.version ? parsed.info : null;
  } catch {
    return null;
  }
}

function writeCache(info: ReleaseInfo) {
  try {
    const payload: CacheShape = { savedAt: new Date().toISOString(), info };
    localStorage.setItem(CACHE_KEY, JSON.stringify(payload));
  } catch {
    // 隐身模式或存储受限：缓存不可用不影响主流程。
  }
}

function parseRelease(d: {
  tag_name?: unknown;
  assets?: Array<{ name: string; browser_download_url: string }>;
  published_at?: unknown;
}): ReleaseInfo | null {
  if (typeof d?.tag_name !== "string") return null;
  const assets = d.assets ?? [];
  const dmg = assets.find((a) => /aarch64\.dmg$/i.test(a.name));
  const windows = assets.find((a) => /_x64-setup\.exe$/i.test(a.name));
  const linux = assets.find((a) => /_linux_x86_64\.tar\.gz$/i.test(a.name));
  const checksums = assets.find((a) => a.name === "SHA256SUMS");
  return {
    version: d.tag_name,
    dmgUrl: dmg?.browser_download_url ?? null,
    windowsUrl: windows?.browser_download_url ?? null,
    linuxUrl: linux?.browser_download_url ?? null,
    checksumsUrl: checksums?.browser_download_url ?? null,
    date: typeof d.published_at === "string" ? d.published_at.slice(0, 10) : null,
  };
}

/** 下载卡片岛（#363 平移自 #349 的 React 下载区）：版本/资产跟随 GitHub release，
 *  状态机区分加载中 / 失败 / 就绪，失败可重试，成功结果本地缓存。 */
export default function DownloadCards({ lang }: { lang: Lang }) {
  const i18n = createI18n(lang);
  const [release, setRelease] = useState<ReleaseInfo | null>(() => readCache());
  const [status, setStatus] = useState<ReleaseStatus>(() => (readCache() ? "ready" : "loading"));
  const [stale, setStale] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let dead = false;
    fetch(API)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((d) => {
        if (dead) return;
        const info = parseRelease(d);
        if (!info) return;
        setRelease(info);
        writeCache(info);
        setStale(false);
        setStatus("ready");
      })
      .catch(() => {
        if (dead) return;
        setStale(true);
        setStatus((s) => (s === "ready" ? "ready" : "failed"));
      });
    return () => {
      dead = true;
    };
  }, [attempt]);

  const retry = useCallback(() => {
    setStale(false);
    setStatus((s) => (s === "ready" ? "ready" : "loading"));
    setAttempt((a) => a + 1);
  }, []);

  const detected = (() => {
    const ua = navigator.userAgent;
    if (/Mac/i.test(ua)) return "macOS";
    if (/Win/i.test(ua)) return "Windows";
    if (/Linux/i.test(ua) && !/Android/i.test(ua)) return "Linux";
    return null;
  })();

  const platforms = [
    { name: "macOS", detail: i18n("download.1"), url: release?.dmgUrl, label: i18n("download.2"), note: i18n("download.3") },
    { name: "Windows", detail: "x64 (Windows 11)", url: release?.windowsUrl, label: i18n("download.4"), note: i18n("download.5") },
    { name: "Linux", detail: i18n("download.6"), url: release?.linuxUrl, label: i18n("download.7"), note: i18n("download.8") },
  ];
  const headline = release
    ? `${release.version}${release.date ? `（${release.date}）` : ""}，${i18n("download.9")}`
    : status === "loading"
      ? i18n("download.10")
      : i18n("download.11");

  return (
    <>
      <div className="section-heading" style={{ marginTop: 12 }}>
        <p style={{ margin: 0 }}>{headline}</p>
      </div>
      {(status === "failed" || stale) && (
        <p className="download-notice" role="status">
          {status === "failed" ? i18n("download.12") : i18n("download.13")}
          <button className="download-retry" onClick={retry}>{i18n("download.14")}</button>
        </p>
      )}
      <div className="download-platforms">
        {platforms.map((p) => (
          <article key={p.name} className={"download-platform" + (detected === p.name ? " recommended" : "")}>
            <span className="platform-detail">{p.detail}</span>
            {detected === p.name && <span className="platform-badge">{i18n("download.15")}</span>}
            <h3>Crossbow for {p.name}</h3>
            <p>{p.note}</p>
            <div className="platform-actions">
              {status === "loading" ? (
                <span className="download-state" role="status">{i18n("download.10")}</span>
              ) : p.url ? (
                <a className="primary-button" href={p.url} target="_blank" rel="noopener noreferrer">{p.label}</a>
              ) : (
                <a className="primary-button" href={LATEST} target="_blank" rel="noopener noreferrer">
                  {_(status === "failed" ? "前往发布页下载" : "该版本暂无此安装包，前往发布页")}
                </a>
              )}
            </div>
          </article>
        ))}
      </div>
      <div className="download-links">
        <a href={LATEST} target="_blank" rel="noopener noreferrer">{i18n("download.16")}</a>
        {release?.checksumsUrl && <a href={release.checksumsUrl} target="_blank" rel="noopener noreferrer">SHA256SUMS ↗</a>}
      </div>
    </>
  );
}
