import { useEffect, useState } from "react";
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

const API = "https://api.github.com/repos/Richard-Zhang1019/Crossbow/releases/latest";

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

function versionNewer(next: string, current: string): boolean {
  const parts = (v: string) => v.replace(/^v/i, "").split(/[.-]/).map((p) => parseInt(p, 10) || 0);
  const a = parts(next);
  const b = parts(current);
  for (let i = 0; i < Math.max(a.length, b.length); i += 1) {
    const diff = (a[i] ?? 0) - (b[i] ?? 0);
    if (diff !== 0) return diff > 0;
  }
  return false;
}

/** 下载卡片岛：版本/资产在构建时注入（`initial`，来自 site-meta），
 *  首屏 HTML 即带可用的版本号与安装包链接——不出现加载占位，
 *  GitHub API 匿名限流也不会卡住下载。浏览器端仅在发现更新的
 *  release 时静默替换，失败则保留静态版本。 */
export default function DownloadCards({ lang, initial }: { lang: Lang; initial: ReleaseInfo }) {
  const i18n = createI18n(lang);
  const [release, setRelease] = useState<ReleaseInfo>(initial);

  useEffect(() => {
    let dead = false;
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(), 8000);
    const run = async () => {
      try {
        const response = await fetch(API, { signal: controller.signal, cache: "no-store" });
        if (!response.ok) throw new Error(String(response.status));
        const info = parseRelease(await response.json());
        if (!dead && info && versionNewer(info.version, initial.version)) setRelease(info);
      } catch {
        // 静态版本仍然可用：API 失败不打扰访客，也不显示错误提示。
      } finally {
        window.clearTimeout(timer);
      }
    };
    void run();
    return () => {
      dead = true;
      controller.abort();
    };
  }, [initial.version]);

  const detected = (() => {
    const ua = navigator.userAgent;
    if (/Mac/i.test(ua)) return "macOS";
    if (/Win/i.test(ua)) return "Windows";
    if (/Linux/i.test(ua) && !/Android/i.test(ua)) return "Linux";
    return null;
  })();

  const platforms = [
    { name: "macOS", detail: i18n("card.1"), url: release.dmgUrl, label: i18n("card.2"), note: i18n("card.3") },
    { name: "Windows", detail: "x64 (Windows 10 / 11)", url: release.windowsUrl, label: i18n("card.4"), note: i18n("card.5") },
    { name: "Linux", detail: i18n("card.6"), url: release.linuxUrl, label: i18n("card.7"), note: i18n("card.8") },
  ];
  const headline = `${release.version}${release.date ? `（${release.date}）` : ""}，${i18n("card.9")}`;

  return (
    <>
      <div className="section-heading" style={{ marginTop: 12 }}>
        <p style={{ margin: 0 }}>{headline}</p>
      </div>
      <div className="download-platforms">
        {platforms.map((p) => (
          <article key={p.name} className={"download-platform" + (detected === p.name ? " recommended" : "")}>
            <span className="platform-detail">{p.detail}</span>
            {detected === p.name && <span className="platform-badge">{i18n("card.15")}</span>}
            <h3>Crossbow for {p.name}</h3>
            <p>{p.note}</p>
            <div className="platform-actions">
              <a className="primary-button" href={p.url ?? LATEST} target="_blank" rel="noopener noreferrer">
                {p.url ? p.label : (lang === "zh" ? "前往发布页下载" : "Download from releases")}
              </a>
            </div>
          </article>
        ))}
      </div>
      <div className="download-links">
        <a href={LATEST} target="_blank" rel="noopener noreferrer">{i18n("card.16")}</a>
        {release.checksumsUrl && <a href={release.checksumsUrl} target="_blank" rel="noopener noreferrer">SHA256SUMS ↗</a>}
      </div>
    </>
  );
}
