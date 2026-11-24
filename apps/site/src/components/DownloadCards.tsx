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

const CACHE_TTL_MS = 10 * 60 * 1000;

interface CachedRelease {
  info: ReleaseInfo;
  fresh: boolean;
}

function readCache(): CachedRelease | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CacheShape;
    if (!parsed?.info?.version) return null;
    const savedAt = Date.parse(parsed.savedAt);
    const fresh = Number.isFinite(savedAt) && Date.now() - savedAt < CACHE_TTL_MS;
    return { info: parsed.info, fresh };
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
  const [release, setRelease] = useState<ReleaseInfo | null>(null);
  const [status, setStatus] = useState<ReleaseStatus>("loading");
  const [stale, setStale] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    // Hydrate from the cache after mount (server HTML must not depend on localStorage),
    // and trust a warm cache for ten minutes; manual retry always refreshes.
    const cached = readCache();
    if (cached) {
      setRelease(cached.info);
      setStatus((s) => (s === "loading" ? "ready" : s));
      if (attempt === 0 && cached.fresh) return;
    }
    let dead = false;
    const run = async (retriesLeft: number) => {
      const controller = new AbortController();
      const timer = window.setTimeout(() => controller.abort(), 8000);
      try {
        const response = await fetch(API, { signal: controller.signal, cache: "no-store" });
        if (!response.ok) throw new Error(String(response.status));
        const info = parseRelease(await response.json());
        if (!info) throw new Error("release payload");
        if (dead) return;
        setRelease(info);
        writeCache(info);
        setStale(false);
        setStatus("ready");
      } catch {
        if (dead) return;
        if (retriesLeft > 0) {
          window.setTimeout(() => {
            if (!dead) void run(retriesLeft - 1);
          }, 1500);
          return;
        }
        setStale(true);
        setStatus((s) => (s === "ready" ? "ready" : "failed"));
      } finally {
        window.clearTimeout(timer);
      }
    };
    void run(1);
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
    { name: "macOS", detail: i18n("card.1"), url: release?.dmgUrl, label: i18n("card.2"), note: i18n("card.3") },
    { name: "Windows", detail: "x64 (Windows 11)", url: release?.windowsUrl, label: i18n("card.4"), note: i18n("card.5") },
    { name: "Linux", detail: i18n("card.6"), url: release?.linuxUrl, label: i18n("card.7"), note: i18n("card.8") },
  ];
  const headline = release
    ? `${release.version}${release.date ? `（${release.date}）` : ""}，${i18n("card.9")}`
    : status === "loading"
      ? i18n("card.10")
      : i18n("card.11");

  return (
    <>
      <div className="section-heading" style={{ marginTop: 12 }}>
        <p style={{ margin: 0 }}>{headline}</p>
      </div>
      {(status === "failed" || stale) && (
        <p className="download-notice" role="status">
          <svg className="download-notice-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7.6v.4" /></svg>
          <span className="download-notice-copy">{status === "failed" ? i18n("card.12") : i18n("card.13")}</span>
          <button className="download-retry" onClick={retry}>{i18n("card.14")}</button>
        </p>
      )}
      <div className="download-platforms">
        {platforms.map((p) => (
          <article key={p.name} className={"download-platform" + (detected === p.name ? " recommended" : "")}>
            <span className="platform-detail">{p.detail}</span>
            {detected === p.name && <span className="platform-badge">{i18n("card.15")}</span>}
            <h3>Crossbow for {p.name}</h3>
            <p>{p.note}</p>
            <div className="platform-actions">
              {status === "loading" ? (
                <span className="download-state" role="status">{i18n("card.10")}</span>
              ) : p.url ? (
                <a className="primary-button" href={p.url} target="_blank" rel="noopener noreferrer">{p.label}</a>
              ) : (
                <a className="primary-button" href={LATEST} target="_blank" rel="noopener noreferrer">
                  {status === "failed"
                    ? (lang === "zh" ? "前往发布页下载" : "Download from releases")
                    : (lang === "zh" ? "该版本暂无此安装包，前往发布页" : "Package missing — open releases")}
                </a>
              )}
            </div>
          </article>
        ))}
      </div>
      <div className="download-links">
        <a href={LATEST} target="_blank" rel="noopener noreferrer">{i18n("card.16")}</a>
        {release?.checksumsUrl && <a href={release.checksumsUrl} target="_blank" rel="noopener noreferrer">SHA256SUMS ↗</a>}
      </div>
    </>
  );
}
