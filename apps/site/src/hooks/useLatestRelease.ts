import { useEffect, useState } from "react";

export interface ReleaseInfo {
  /** 形如 v0.5.0 */
  version: string;
  /** Apple Silicon dmg 资产直链（该版本含此资产时才有） */
  dmgUrl: string | null;
  /** 发布日期 YYYY-MM-DD */
  date: string | null;
}

const API = "https://api.github.com/repos/Richard-Zhang1019/Crossbow/releases/latest";

/** 运行时拉取最新 release：版本号/dmg 链接/发布日期自动跟随 GitHub，
 *  无需发版后手动改官网。请求失败（限流/离线）返回 null，由调用方回落静态值。 */
export function useLatestRelease(): ReleaseInfo | null {
  const [info, setInfo] = useState<ReleaseInfo | null>(null);

  useEffect(() => {
    let dead = false;
    fetch(API)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
      .then((d) => {
        if (dead || typeof d.tag_name !== "string") return;
        const assets: Array<{ name: string; browser_download_url: string }> = d.assets ?? [];
        const dmg = assets.find((a) => /aarch64\.dmg$/i.test(a.name));
        setInfo({
          version: d.tag_name,
          dmgUrl: dmg ? dmg.browser_download_url : null,
          date: typeof d.published_at === "string" ? d.published_at.slice(0, 10) : null,
        });
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, []);

  return info;
}
