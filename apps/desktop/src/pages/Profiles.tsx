import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ipc, type Profile } from "../ipc";
import { useT } from "../i18n";

function fmtBytesLocal(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  return `${(n / 1024 ** i).toFixed(1)} ${units[i]}`;
}

function fmtTime(ts: number | null | undefined, t: (k: "profiles.justNow" | "profiles.minAgo" | "profiles.hourAgo", p?: Record<string, number>) => string): string {
  if (!ts) return "—";
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return t("profiles.justNow");
  if (diff < 3600) return t("profiles.minAgo", { n: Math.floor(diff / 60) });
  if (diff < 86400) return t("profiles.hourAgo", { n: Math.floor(diff / 3600) });
  return new Date(ts * 1000).toLocaleDateString();
}

/** 配置页：订阅导入（URL）、更新、激活、删除；覆写链在 M0-S3 接入。 */
export default function ProfilesPage() {
  const t = useT();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const refresh = useCallback(async () => {
    setProfiles(await ipc.listProfiles().catch(() => []));
    setActiveId(await ipc.activeProfileId().catch(() => null));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const importPaths = useCallback(
    async (paths: string[]) => {
      for (const p of paths) {
        await run2(p, () => ipc.importProfileFile(p));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  useEffect(() => {
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragOver(true);
        } else if (event.payload.type === "leave") {
          setDragOver(false);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          void importPaths(event.payload.paths);
        }
      })
      .then((u) => (un = u));
    return () => un?.();
  }, [importPaths]);

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
  const run2 = run;

  const chosenFiles = async (files: FileList | null) => {
    if (!files?.length) return;
    for (const f of Array.from(files)) {
      const text = await f.text();
      const name = f.name.replace(/\.[^.]+$/, "");
      await run(f.name, () => ipc.importProfileContent(name, text));
    }
  };

  return (
    <div className="mx-auto max-w-4xl space-y-5">
      <h1 className="text-lg font-semibold">{t("profiles.title")}</h1>

      <div
        className="flex gap-2 rounded-xl p-2 transition-colors"
        style={{
          background: "var(--cb-surface)",
          border: `2px ${dragOver ? "dashed var(--cb-accent)" : "solid transparent"}`,
        }}
      >
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
          placeholder={t("profiles.urlPlaceholder")}
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
          {busy === "import" ? t("profiles.importing") : t("profiles.import")}
        </button>
        <button
          disabled={busy !== null}
          onClick={() => fileInputRef.current?.click()}
          className="rounded-lg border px-3 py-2 text-xs disabled:opacity-40"
          style={{ borderColor: "var(--cb-border)" }}
        >
          {busy?.endsWith(".yaml") || busy?.endsWith(".yml")
            ? t("profiles.importing")
            : t("profiles.importFile")}
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept=".yaml,.yml,.txt,.conf"
          multiple
          hidden
          onChange={(e) => {
            void chosenFiles(e.target.files);
            e.target.value = "";
          }}
        />
      </div>
      <div className="text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
        {t("profiles.dropHint")}
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
          {t("profiles.empty")}
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
  const t = useT();
  const traffic = profile.traffic;
  const used = traffic ? traffic.upload + traffic.download : 0;
  const pct = traffic && traffic.total > 0 ? Math.min(100, (used / traffic.total) * 100) : 0;

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
                {t("profiles.current")}
              </span>
            )}
          </div>
          <div className="mt-1 text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {profile.kind === "remote" ? t("profiles.remote") : t("profiles.local")} ·{" "}
            {t("profiles.updatedAt", { time: fmtTime(profile.last_updated, t) })}
          </div>

          {traffic && traffic.total > 0 && (
            <div className="mt-2">
              <div className="h-1.5 w-full overflow-hidden rounded-full"
                style={{ background: "var(--cb-border)" }}>
                <div className="h-full rounded-full"
                  style={{ width: `${pct}%`, background: pct > 90 ? "#e0524c" : "var(--cb-accent)" }} />
              </div>
              <div className="mt-1 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
                ↑{fmtBytesLocal(traffic.upload)} ↓{fmtBytesLocal(traffic.download)} / {fmtBytesLocal(traffic.total)}
              </div>
            </div>
          )}
        </div>

        <div className="flex shrink-0 gap-2 text-xs">
          {profile.kind === "remote" && (
            <button disabled={busy} onClick={onUpdate}
              className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
              style={{ borderColor: "var(--cb-border)" }}>
              {busy ? "…" : t("profiles.update")}
            </button>
          )}
          {!active && (
            <button disabled={busy} onClick={onActivate}
              className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
              style={{ borderColor: "var(--cb-border)" }}>
              {t("profiles.setActive")}
            </button>
          )}
          <button disabled={busy} onClick={onRemove}
            className="rounded-lg border px-3 py-1.5 disabled:opacity-40"
            style={{ borderColor: "var(--cb-border)", color: "#e0524c" }}>
            {t("profiles.remove")}
          </button>
        </div>
      </div>
    </section>
  );
}
