import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ipc, type Profile } from "../ipc";
import Select from "../components/Select";
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

  // 后台调度器完成自动更新后刷新列表
  useEffect(() => {
    const un = ipc.onProfileUpdated(() => refresh());
    return () => void un.then((f) => f());
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
      <h1 className="text-[15px] font-semibold tracking-tight">{t("profiles.title")}</h1>

      <div
        className="cb-card flex gap-2 p-2 transition-colors"
        style={{
          border: dragOver
            ? "1px dashed var(--cb-accent-line)"
            : "1px solid transparent",
          background: dragOver ? "var(--cb-accent-soft)" : undefined,
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
          className="cb-input cb-selectable flex-1 px-3 py-2"
        />
        <button
          disabled={!url || busy === "import"}
          onClick={() =>
            run("import", async () => {
              await ipc.importProfileUrl(url);
              setUrl("");
            })
          }
          className="cb-btn acc px-4 py-2"
        >
          {busy === "import" ? t("profiles.importing") : t("profiles.import")}
        </button>
        <button
          disabled={busy !== null}
          onClick={() => fileInputRef.current?.click()}
          className="cb-btn px-3 py-2"
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
          style={{
            borderColor: "color-mix(in srgb, var(--cb-bad) 40%, transparent)",
            color: "var(--cb-bad)",
          }}>
          {error}
        </div>
      )}

      {profiles.length === 0 ? (
        <div className="cb-card flex h-48 items-center justify-center text-sm"
          style={{ color: "var(--cb-text-dim)" }}>
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
              onSetInterval={(min) => run(p.id, () => ipc.setProfileInterval(p.id, min))}
              onRename={(name) => run(p.id, () => ipc.renameProfile(p.id, name))}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function nextUpdateText(profile: Profile, t: ReturnType<typeof useT>): string {
  const lu = profile.last_update;
  if (lu && !lu.ok) {
    return t("profiles.updateFailedAt", { detail: (lu.detail ?? "?").slice(0, 80) });
  }
  const interval = profile.update_interval_min * 60;
  const last = profile.last_updated ?? 0;
  const next = last + interval;
  const time = new Date(next * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
  return t("profiles.nextAt", { time });
}

function ProfileCard({
  profile,
  active,
  busy,
  onUpdate,
  onActivate,
  onRemove,
  onSetInterval,
  onRename,
}: {
  profile: Profile;
  active: boolean;
  busy: boolean;
  onUpdate: () => void;
  onActivate: () => void;
  onRemove: () => void;
  onSetInterval: (min: number) => void;
  onRename: (name: string) => void;
}) {
  const t = useT();
  const [editing, setEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState(profile.name);
  const traffic = profile.traffic;
  const used = traffic ? traffic.upload + traffic.download : 0;
  const pct = traffic && traffic.total > 0 ? Math.min(100, (used / traffic.total) * 100) : 0;

  return (
    <section
      className="cb-card p-4"
      style={{
        borderColor: active ? "var(--cb-accent-line)" : "var(--cb-line)",
        background: active
          ? "linear-gradient(180deg, var(--cb-accent-soft), transparent), var(--cb-surface)"
          : undefined,
      }}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            {editing ? (
              <input
                autoFocus
                value={nameDraft}
                onChange={(e) => setNameDraft(e.target.value)}
                onBlur={() => {
                  setEditing(false);
                  if (nameDraft.trim() && nameDraft !== profile.name) {
                    onRename(nameDraft.trim());
                  }
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  if (e.key === "Escape") {
                    setNameDraft(profile.name);
                    setEditing(false);
                  }
                }}
                placeholder={t("profiles.namePlaceholder")}
                className="cb-input cb-selectable py-0.5 text-sm"
                style={{ width: 260 }}
              />
            ) : (
              <span className="text-sm font-medium cb-selectable">{profile.name}</span>
            )}
            <button
              onClick={() => {
                setNameDraft(profile.name);
                setEditing(true);
              }}
              title={t("profiles.rename")}
              className="text-[11px]"
              style={{ color: "var(--cb-faint)" }}
            >
              ✎
            </button>
            {active && (
              <span className="cb-badge">{t("profiles.current")}</span>
            )}
          </div>
          <div className="mt-1 flex items-center gap-2 text-xs" style={{ color: "var(--cb-text-dim)" }}>
            {profile.kind === "remote" ? t("profiles.remote") : t("profiles.local")} ·{" "}
            {t("profiles.updatedAt", { time: fmtTime(profile.last_updated, t) })}
            {profile.kind === "remote" && (
              <Select
                value={profile.update_interval_min}
                options={[
                  { value: 0, label: t("profiles.autoOff") },
                  { value: 360, label: t("profiles.autoH", { n: 6 }) },
                  { value: 720, label: t("profiles.autoH", { n: 12 }) },
                  { value: 1440, label: t("profiles.autoH", { n: 24 }) },
                ]}
                onChange={(v) => onSetInterval(Number(v))}
              />
            )}
          </div>
          {profile.kind === "remote" && profile.update_interval_min > 0 && (
            <div className="mt-1 text-[11px]" style={{ color: "var(--cb-faint)" }}>
              {nextUpdateText(profile, t)}
            </div>
          )}

          {traffic && traffic.total > 0 && (
            <div className="mt-2">
              <div className="h-[3px] w-full overflow-hidden rounded-full"
                style={{ background: "color-mix(in srgb, var(--cb-text) 7%, transparent)" }}>
                <div className="h-full rounded-full"
                  style={{ width: `${pct}%`, background: pct > 90 ? "var(--cb-bad)" : "linear-gradient(90deg, var(--cb-accent), #9d8cff)" }} />
              </div>
              <div className="mt-1 text-[11px]" style={{ color: "var(--cb-text-dim)" }}>
                ↑{fmtBytesLocal(traffic.upload)} ↓{fmtBytesLocal(traffic.download)} / {fmtBytesLocal(traffic.total)}
              </div>
            </div>
          )}
        </div>

        <div className="flex shrink-0 gap-2 text-xs">
          {profile.kind === "remote" && (
            <button disabled={busy} onClick={onUpdate} className="cb-btn py-1.5">
              {busy ? "…" : t("profiles.update")}
            </button>
          )}
          {!active && (
            <button disabled={busy} onClick={onActivate} className="cb-btn py-1.5">
              {t("profiles.setActive")}
            </button>
          )}
          <button disabled={busy} onClick={onRemove} className="cb-btn py-1.5"
            style={{ color: "var(--cb-bad)" }}>
            {t("profiles.remove")}
          </button>
        </div>
      </div>
    </section>
  );
}
