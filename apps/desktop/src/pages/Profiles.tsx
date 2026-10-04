import { useCallback, useEffect, useState } from "react";
import { ipc, type Profile } from "../ipc";

function fmtBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  return `${(n / 1024 ** i).toFixed(1)} ${units[i]}`;
}

function fmtTime(ts?: number | null): string {
  if (!ts) return "未更新";
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return new Date(ts * 1000).toLocaleDateString();
}

/** 配置页：订阅导入（URL）、更新、激活、删除；覆写链在 M0-S3 接入。 */
export default function ProfilesPage() {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const run = async (key: string, fn: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <h1 className="text-lg font-semibold">配置</h1>

      <div className="flex gap-2">
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && url) {
              run("import", async () => {
                await ipc.importProfileUrl(url);
                setUrl("");
              });
            }
          }}
          placeholder="订阅 URL，回车或点击导入"
          className="cb-selectable flex-1 rounded-lg border px-3 py-2 text-sm outline-none focus:border-[var(--cb-accent)]"
          style={{ borderColor: "var(--cb-border)", background: "var(--cb-surface)" }}
        />
        <button
          disabled={!url || busy === "import"}
          onClick={() =>
            run("import", async () => {
              await ipc.importProfileUrl(url);
              setUrl("");
            })
          }
          className="rounded-lg px-4 py-2 text-sm font-medium text-white disabled:opacity-40"
          style={{ background: "var(--cb-accent)" }}
        >
          {busy === "import" ? "导入中…" : "＋ 导入"}
        </button>
      </div>

      {error && (
        <div className="rounded-lg border px-3 py-2 text-xs"
          style={{ borderColor: "#e0524c", color: "#e0524c" }}>
          {error}
        </div>
      )}

      {profiles.length === 0 ? (
        <div className="flex h-48 items-center justify-center rounded-xl border text-sm"
          style={{ background: "var(--cb-surface)", borderColor: "var(--cb-border)", color: "var(--cb-text-dim)" }}>
          还没有配置，粘贴订阅 URL 开始
        </div>
      ) : (
        <div className="space-y-3">
          {profiles.map((p) => (
            <ProfileCard
              key={p.id}
              profile={p}
              active={p.id === activeId}
              busy={busy === p.id}
              onUpdate={() => run(p.id, () => ipc.updateProfile(p.id))}
              onActivate={() => run(p.id, () => ipc.setActiveProfile(p.id))}
              onRemove={() => run(p.id, () => ipc.removeProfile(p.id))}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function ProfileCard({
  profile,
  active,
  busy,
  onUpdate,
  onActivate,
  onRemove,
}: {
  profile: Profile;
  active: boolean;
  busy: boolean;
  onUpdate: () => void;
  onActivate: () => void;
  onRemove: () => void;
}) {
  const t = profile.traffic;
  const used = t ? t.upload + t.download : 0;
  const pct = t && t.total > 0 ? Math.min(100, (used / t.total) * 100) : 0;

  return (
    <section
      className="rounded-xl border p-4"
      style={{
        background: "var(--cb-surface)",
        borderColor: active ? "var(--cb-accent)" : "var(--cb-border)",
        borderWidth: active ? 2 : 1,
      }}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium cb-selectable">{profile.name}</span>
            {active && (
              <span className="rounded-full px-2 py-0.5 text-[10px] font-medium text-white"
                style={{ background: "var(--cb-accent)" }}>
                当前
              </span>
            )}
          </div>
          <div className="mt-1 text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {profile.kind === "remote" ? "订阅" : "本地"} · 更新于 {fmtTime(profile.last_updated)}
          </div>

          {t && t.total > 0 && (
            <div className="mt-2">
              <div className="h-1.5 w-full overflow-hidden rounded-full"
                style={{ background: "var(--cb-border)" }}>
                <div className="h-full rounded-full"
                  style={{ width: `${pct}%`, background: pct > 90 ? "#e0524c" : "var(--cb-accent)" }} />
              </div>
              <div className="mt-1 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
                ↑{fmtBytes(t.upload)} ↓{fmtBytes(t.download)} / {fmtBytes(t.total)}
              </div>
            </div>
          )}
        </div>

        <div className="flex shrink-0 gap-2 text-xs">
          {profile.kind === "remote" && (
            <button disabled={busy} onClick={onUpdate}
              className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
              style={{ borderColor: "var(--cb-border)" }}>
              {busy ? "…" : "更新"}
            </button>
          )}
          {!active && (
            <button disabled={busy} onClick={onActivate}
              className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
              style={{ borderColor: "var(--cb-border)" }}>
              设为当前
            </button>
          )}
          <button disabled={busy} onClick={onRemove}
            className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
            style={{ borderColor: "var(--cb-border)", color: "#e0524c" }}>
            删除
          </button>
        </div>
      </div>
    </section>
  );
}
