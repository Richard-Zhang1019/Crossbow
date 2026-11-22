import ImportCompatibility from "./ImportCompatibility";
import {Input} from "@/components/Controls";
import type { NavigationTarget } from "@/lib/workflow";
import ConfigRecovery from "@/components/ConfigRecovery";
import { Button, ButtonBase } from "@/components/Button";
import { subscriptionCleanup } from "@/lib/unlisten";
import { notify, notificationText } from "@/notifications/service";
import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ipc, type PackagePreview, type Profile, type ProfileStatus } from "@/services/ipc";
import Select from "@/components/Select";
import OverridePanel from "@/pages/OverridePanel";
import RuleSets from "@/components/RuleSets";
import ProcessRules from "@/components/ProcessRules";
import { useT } from "@/i18n/i18n";
import FitItems from "@/components/FitItems";
import Dialog from "@/components/Dialog";
import { Icon } from "@/components/Icon";
import PopoverPanel from "@/components/PopoverPanel";
import { ProgressRing } from "@/components/ProgressRing";
import { PageHead } from "@/pages/Home";
import { writeSetupProgress } from "@/pages/Home/firstConnection";

function fmtBytesLocal(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(n) / Math.log(1024)),
    units.length - 1,
  );
  return `${(n / 1024 ** i).toFixed(1)} ${units[i]}`;
}

function fmtTime(
  ts: number | null | undefined,
  t: (
    k: "profiles.justNow" | "profiles.minAgo" | "profiles.hourAgo",
    p?: Record<string, number>,
  ) => string,
): string {
  if (!ts) return "—";
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return t("profiles.justNow");
  if (diff < 3600) return t("profiles.minAgo", { n: Math.floor(diff / 60) });
  if (diff < 86400)
    return t("profiles.hourAgo", { n: Math.floor(diff / 3600) });
  return new Date(ts * 1000).toLocaleDateString();
}

/** 配置页：订阅导入（URL）、更新、激活、删除；覆写链在 M0-S3 接入。 */
export default function ProfilesPage({
  tab = "sources", focusProfileId, navigationRequest,
}: {
  tab?: "sources" | "overrides" | "process" | "rulesets";
  focusProfileId?:string;
  navigationRequest?:NavigationTarget|null;
}) {
  const t = useT();
  const [recovering,setRecovering]=useState(false);
  const [creatingOverride,setCreatingOverride]=useState(false);
  const [statuses,setStatuses]=useState<ProfileStatus[]>([]);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [url, setUrl] = useState("");
  const [canImport, setCanImport] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [pkgBusy, setPkgBusy] = useState<"export" | "preview" | "import" | null>(null);
  const [exportingPackage, setExportingPackage] = useState(false);
  const [pkgPreview, setPkgPreview] = useState<PackagePreview | null>(null);
  const packageOperation = useRef(false);
  const mounted = useRef(false);
  const readRevision = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; ++readRevision.current; };
  }, []);
  const previewId = pkgPreview?.id;
  useEffect(() => {
    if (previewId) return () => { void ipc.cancelConfigPackage(previewId).catch(() => {}); };
  }, [previewId]);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const importingFiles = useRef(false);
  useEffect(() => {
    if (focusProfileId) setSelectedId(focusProfileId);
    if (navigationRequest?.control === "cb-config-recovery") setRecovering(true);
    if (navigationRequest?.control === "cb-import-profile") setImporting(true);
  }, [focusProfileId, navigationRequest]);

  const refresh = useCallback(async () => {
    if (!mounted.current) return;
    const revision = ++readRevision.current;
    const results=await Promise.allSettled([ipc.listProfiles(),ipc.activeProfileId(),ipc.profileStatuses()]);
    if (!mounted.current || revision !== readRevision.current) return;
    const [profiles,active,statuses]=results;
    if(profiles.status==='fulfilled')setProfiles(profiles.value);
    if(active.status==='fulfilled')setActiveId(active.value);
    if(statuses.status==='fulfilled')setStatuses(statuses.value);
    if(results.some(r=>r.status==='rejected'))notify.error(results.filter(r=>r.status==='rejected').map(r=>(r as PromiseRejectedResult).reason).join("; "),"profiles-read");
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 后台调度器完成自动更新后刷新列表
  useEffect(() => {
    const un = ipc.onProfileUpdated(() => refresh());
    return subscriptionCleanup(un);
  }, [refresh]);

  const importProfiles = useCallback(async (tasks: Array<() => Promise<Profile>>) => {
    if (!tasks.length || importingFiles.current) return false;
    importingFiles.current = true;
    setBusy("import");
    try {
      let allSucceeded = true;
      for (const [index, task] of tasks.entries()) {
        // A transient read failure should still let the user import the profile.
        const previous = await ipc.activeProfileId().catch(() => undefined);
        const result = await notify.run(`profile-import-${index}`, "", task, notificationText("import.done"), {
          titleKey: "profiles.title",
          nextActions: previous === null ? ["continueSetup"] : ["nodes"],
        });
        if (!result.ok) allSucceeded = false;
        else if (previous === null) {
          writeSetupProgress("pending");
          window.dispatchEvent(new CustomEvent("cb:profile-switched"));
        }
      }
      await refresh();
      if (mounted.current && allSucceeded) setImporting(false);
      return allSucceeded;
    } finally {
      importingFiles.current = false;
      if (mounted.current) setBusy(null);
    }
  }, [refresh]);

  useEffect(() => {
    let un: (() => void) | undefined;
    let disposed = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragOver(true);
        } else if (event.payload.type === "leave") {
          setDragOver(false);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          void importProfiles(event.payload.paths.map(path => () => ipc.importProfileFile(path)));
        }
      })
      .then((u) => {
        if (disposed) u();
        else un = u;
      })
      .catch((error) => notify.error(error, "profile-file-import"));
    return () => {
      disposed = true;
      un?.();
    };
  }, [importProfiles]);

  const run = async (
    key: string,
    fn: () => Promise<unknown>,
  ) => {
    setBusy(key);
    try {
      const outcome=await notify.run(`profile-${key}`, t("profiles.title"), fn,
        undefined,
        {
        feedback: /^(update|activate|remove|interval|rename)-/.test(key)
          ? "error-only"
          : "task",
        titleKey: "profiles.title",
      });
      if(outcome.ok&&key.startsWith("activate-"))window.dispatchEvent(new CustomEvent("cb:profile-switched"));
      if (!mounted.current) return;
      await refresh();
    } finally {
      if (mounted.current) setBusy(null);
    }
  };

  const runPackageOperation = async (
    operation: "export" | "preview" | "import",
    run: () => Promise<void>,
  ) => {
    if (packageOperation.current) return;
    packageOperation.current = true;
    setPkgBusy(operation);
    try {
      await run();
    } catch (error) {
      const code = String(error);
      notify.error(code.startsWith("pkg.error") ? t(code) : t("pkg.errorCommit"), `pkg-${operation}`);
    } finally {
      packageOperation.current = false;
      if (mounted.current) setPkgBusy(null);
    }
  };
  const exportPackage = () => runPackageOperation("export", async () => {
    const saved = await ipc.exportConfigPackage();
    if (mounted.current) setExportingPackage(false);
    if (saved) notify.success(t("pkg.exported"), "pkg-export");
  });
  const importPackage = () => runPackageOperation("preview", async () => {
    const preview = await ipc.previewConfigPackage();
    if (!preview) return;
    if (!mounted.current) {
      await ipc.cancelConfigPackage(preview.id);
      return;
    }
    setPkgPreview(preview);
  });
  const confirmImport = () => runPackageOperation("import", async () => {
    if (!pkgPreview) return;
    try {
      await ipc.importConfigPackage(pkgPreview.id);
      notify.success(t("pkg.imported"), "pkg-import");
    } finally {
      // The backend consumes the preview even if commit fails. Retry starts
      // with a fresh preview and current conflict counts.
      if (mounted.current) setPkgPreview(null);
    }
  });
  const closeExportPackage = () => {
    if (!packageOperation.current) setExportingPackage(false);
  };
  const closeImportPackage = () => {
    if (!packageOperation.current) setPkgPreview(null);
  };
  const chosenFiles = (files: FileList | null) => {
    if (!files?.length) return;
    void importProfiles(Array.from(files, file => async () =>
      ipc.importProfileContent(file.name.replace(/\.[^.]+$/, ""), await file.text()),
    ));
  };

  const selected =
    profiles.find((p) => p.id === selectedId) ??
    profiles.find((p) => p.id === activeId) ??
    profiles[0];
  useEffect(()=>{
    if(focusProfileId&&selected?.id===focusProfileId){const frame=requestAnimationFrame(()=>document.getElementById("cb-profile-detail")?.focus());return()=>cancelAnimationFrame(frame);}
  },[focusProfileId,selected?.id]);
  const importUrl = async () => {
    if (!canImport) return;
    if (await importProfiles([() => ipc.importProfileUrl(url.trim())]) && mounted.current) setUrl("");
  };
  return (
    <div className="cb-profiles-page">
      <PageHead
        title={t(
          tab === "overrides"
            ? "studio.overrides"
            : tab === "process"
              ? "studio.processRules"
              : tab === "rulesets" ? "rs.title" : "studio.sources",
        )}
        meta={t(tab === "overrides" ? "ov.hint" : tab === "process" ? "pr.hint" : tab === "rulesets" ? "rs.hint" : "studio.configHint")}
      >
        {tab === "overrides" && <Button variant="primary" onClick={()=>setCreatingOverride(true)}>{t("ov.create")}</Button>}
        {tab === "sources" && (
          <>
            {/* #538：低频操作（换机迁移 ×2、历史回滚）收进「备份与迁移」，
                主链路「添加订阅」作为唯一 primary 不与低频按钮平铺。 */}
            <PopoverPanel
              label={t("settings.backupMigrate")}
              trigger={
                <Button className="cb-backup-trigger" disabled={pkgBusy !== null}>
                  {t("settings.backupMigrate")}
                  <Icon name="chevron" />
                </Button>
              }
            >
              {(close) => (
                <div className="cb-backup-menu">
                  <ButtonBase
                    className="cb-backup-menu-item"
                    onClick={() => { close(); setExportingPackage(true); }}
                    disabled={pkgBusy !== null}
                  >
                    <span className="cb-backup-menu-icon"><Icon name="download" size={14} /></span>
                    <span className="cb-backup-menu-copy">
                      <span className="cb-backup-menu-title">{t("backup.exportEntry")}</span>
                      <span className="cb-backup-menu-hint">{t("backup.exportEntryHint")}</span>
                    </span>
                  </ButtonBase>
                  <ButtonBase
                    className="cb-backup-menu-item"
                    onClick={() => { close(); void importPackage(); }}
                    disabled={pkgBusy !== null}
                  >
                    <span className="cb-backup-menu-icon"><Icon name="upload" size={14} /></span>
                    <span className="cb-backup-menu-copy">
                      <span className="cb-backup-menu-title">{t("backup.importEntry")}</span>
                      <span className="cb-backup-menu-hint">{t("backup.importEntryHint")}</span>
                    </span>
                  </ButtonBase>
                  <div className="cb-backup-menu-sep" />
                  <ButtonBase
                    className="cb-backup-menu-item"
                    onClick={() => { close(); setRecovering(true); }}
                  >
                    <span className="cb-backup-menu-icon"><Icon name="refresh" size={14} /></span>
                    <span className="cb-backup-menu-copy">
                      <span className="cb-backup-menu-title">{t("backup.title")}</span>
                      <span className="cb-backup-menu-hint">{t("backup.menuHint")}</span>
                    </span>
                  </ButtonBase>
                </div>
              )}
            </PopoverPanel>
            <Button variant="primary" onClick={() => setImporting(true)}>
              <Icon name="download" />
              {t("studio.importSource")}
            </Button>
          </>
        )}
      </PageHead>
      {recovering&&<ConfigRecovery onClose={()=>setRecovering(false)} onRestored={()=>void refresh()}/>}
      {exportingPackage && (
        <Dialog title={t("pkg.export")} onClose={closeExportPackage}>
          <p className="cb-control-note">{t("pkg.exportHint")}</p>
          <div className="actions">
            <Button onClick={closeExportPackage} disabled={pkgBusy !== null}>{t("ov.cancel")}</Button>
            <Button variant="primary" loading={pkgBusy === "export"} onClick={() => void exportPackage()}>
              {t("pkg.chooseSave")}
            </Button>
          </div>
        </Dialog>
      )}
      {pkgPreview && (
        <Dialog title={t("pkg.import")} onClose={closeImportPackage}>
          <p className="cb-control-note">
            {t("pkg.report", {
              added: pkgPreview.report.added_profiles,
              replaced: pkgPreview.report.replaced_profiles,
              addedOv: pkgPreview.report.added_overrides,
              replacedOv: pkgPreview.report.replaced_overrides,
            })}
          </p>
          <p className="cb-control-note">{t("pkg.policiesReport",{process:pkgPreview.report.added_process_rules??0,replacedProcess:pkgPreview.report.replaced_process_rules??0,sets:pkgPreview.report.added_rule_sets??0,replacedSets:pkgPreview.report.replaced_rule_sets??0})}</p>
          {!!pkgPreview.report.same_name_items&&<p className="cb-control-note">{t("pkg.sameNames",{n:pkgPreview.report.same_name_items})}</p>}
          {!!pkgPreview.report.unverified_targets&&<p className="cb-control-note">{t("pkg.targetsPending",{n:pkgPreview.report.unverified_targets})}</p>}
          {!!pkgPreview.report.policy_conflicts&&<p role="alert" className="cb-control-note">{t("pkg.conflicts",{n:pkgPreview.report.policy_conflicts})}</p>}
          <details><summary>{t("pkg.scope")}</summary><p className="cb-control-note">{t("pkg.range")}</p><p className="cb-control-note">{t("pkg.importHint")}</p></details>
          <p className="cb-control-note">
            {t(pkgPreview.report.selects_profile ? "pkg.selectsProfile" : "pkg.keepsProfile")}
          </p>
          <div className="actions">
            <Button onClick={closeImportPackage} disabled={pkgBusy !== null}>{t("ov.cancel")}</Button>
            <Button variant="primary" disabled={!!pkgPreview.report.policy_conflicts} loading={pkgBusy === "import"} onClick={() => void confirmImport()}>
              {t("pkg.import")}
            </Button>
          </div>
        </Dialog>
      )}
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
      {dragOver && (
        <div className="cb-drop-indicator">{t("profiles.dropHint")}</div>
      )}
      {tab === "overrides" ? (
        <OverridePanel
          createOpen={creatingOverride}
          onCreateClose={()=>setCreatingOverride(false)}
          profiles={profiles}
          activeId={activeId}
          onChanged={refresh}
        />
      ) : tab === "rulesets" ? <RuleSets /> : tab === "process" ? (
        <ProcessRules
          presetProcess={
            navigationRequest?.control === "cb-process-rule"
              ? navigationRequest?.source
              : undefined
          }
        />
      ) : profiles.length === 0 ? (
        <div className="cb-empty">
          <Icon name="document" size={26} />
          <h2>{t("profiles.empty")}</h2>
          <p>{t("profiles.dropHint")}</p>
          <Button className="mt-4" onClick={() => setImporting(true)}>
            {t("studio.importSource")}
          </Button>
        </div>
      ) : (
        <div className="cb-context-layout">
          <nav className="cb-context-nav" aria-label={t("studio.sources")}>
            <h2>{t("studio.sources")}</h2>
            <FitItems
              items={profiles}
              rowHeight={64}
              label={t("studio.sources")}
              render={(profile) => (
                <ButtonBase
                  key={profile.id}
                  className={`cb-profile-list-item ${profile.id === selected?.id ? "active" : ""}`}
                  aria-current={
                    profile.id === selected?.id ? "page" : undefined
                  }
                  onClick={() => setSelectedId(profile.id)}
                >
                  <span>{profile.name}</span>
                  <small>
                    {profile.kind === "remote"
                      ? t("profiles.remote")
                      : t("profiles.local")}
                    {profile.id === activeId
                      ? ` · ${t("profiles.current")}`
                      : ""}
                  </small>
                </ButtonBase>
              )}
            />
          </nav>
          <div className="cb-profile-detail" id="cb-profile-detail" tabIndex={-1}>
            {selected && (
              <ProfileCard
                key={selected.id}
                profile={selected}
                status={statuses.find(s=>s.id===selected.id)}
                active={selected.id === activeId}
                busy={busy !== null}
                onUpdate={() =>
                  run(`update-${selected.id}`, () =>
                    ipc.updateProfile(selected.id),
                  )
                }
                onActivate={() =>
                  run(`activate-${selected.id}`, () =>
                    ipc.setActiveProfile(selected.id),
                  )
                }
                onRemove={() =>
                  run(`remove-${selected.id}`, () =>
                    ipc.removeProfile(selected.id),
                  )
                }
                onSetInterval={(min) =>
                  run(`interval-${selected.id}`, () =>
                    ipc.setProfileInterval(selected.id, min),
                  )
                }
                onRename={(name) =>
                  run(`rename-${selected.id}`, () =>
                    ipc.renameProfile(selected.id, name),
                  )
                }
              />
            )}
          </div>
        </div>
      )}
      {importing && (
        <Dialog
          title={t("studio.importSource")}
          onClose={() => setImporting(false)}
        >
          <p className="cb-control-note">{t("profiles.dropHint")}</p>
          <label htmlFor="profile-url">{t("profiles.sourceLabel")}</label>
          <p id="profile-source-hint" className="cb-control-note">{t("profiles.sourceHint")}</p>
          <textarea
            id="profile-url"
            rows={4}
            aria-describedby="profile-source-hint"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            style={{ width: "100%", resize: "vertical", minHeight: 96, maxHeight: 240 }}
            value={url}
            onChange={(e) => {setCanImport(false); setUrl(e.target.value);}}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && canImport && url.trim() && busy === null) {
                e.preventDefault();
                void importUrl();
              }
            }}
            className="cb-input cb-selectable"
          />
          <ImportCompatibility content={url} onReady={setCanImport} />
          <div className="actions">
            <Button
              disabled={busy !== null}
              onClick={() => fileInputRef.current?.click()}
            >
              {t("profiles.importFile")}
            </Button>
            <Button variant="primary"
              disabled={!canImport || !url.trim() || busy !== null}
              onClick={importUrl}
            >
              {busy === "import"
                ? t("profiles.importing")
                : t("profiles.import")}
            </Button>
          </div>
        </Dialog>
      )}
    </div>
  );
}


function ProfileCard({
  profile,
  status,
  active,
  busy,
  onUpdate,
  onActivate,
  onRemove,
  onSetInterval,
  onRename,
}: {
  profile: Profile;
  status?:ProfileStatus;
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
  const cancelRename = useRef(false);
  const [nameDraft, setNameDraft] = useState(profile.name);
  const traffic = profile.traffic;
  const used = traffic ? traffic.upload + traffic.download : 0;
  const pct =
    traffic && traffic.total > 0
      ? Math.min(100, (used / traffic.total) * 100)
      : 0;

  return (
    <section>
      <div className="flex flex-wrap items-center gap-3">
        {editing ? (
          <Input
            autoFocus
            className="cb-input cb-selectable"
            value={nameDraft}
            onChange={(e) => setNameDraft(e.target.value)}
            onBlur={() => {
              setEditing(false);
              if (cancelRename.current) {
                cancelRename.current = false;
                return;
              }
              if (nameDraft.trim() && nameDraft !== profile.name)
                onRename(nameDraft.trim());
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") {
                cancelRename.current = true;
                setNameDraft(profile.name);
                setEditing(false);
              }
            }}
          />
        ) : (
          <h2 className="text-xl font-medium cb-selectable">{profile.name}</h2>
        )}
        <Button
          disabled={busy}
          onClick={() => {
            cancelRename.current = false;
            setNameDraft(profile.name);
            setEditing(true);
          }}
        >
          {t("profiles.rename")}
        </Button>
        {active && <span className="cb-badge">{t("profiles.current")}</span>}
      </div>
      <dl className="cb-profile-facts">
        <div>
          <dt>{t("studio.sources")}</dt>
          <dd>
            {profile.kind === "remote"
              ? t("profiles.remote")
              : t("profiles.local")}
          </dd>
        </div>
        <div>
          <dt>{t("profiles.updatedAt", { time: "" })}</dt>
          <dd>{fmtTime(profile.last_updated, t)}</dd>
        </div>
        <div>
          <dt>{t("studio.overrides")}</dt>
          <dd>{profile.override_ids.length}</dd>
        </div>
        <div>
          <dt>{t("profiles.current")}</dt>
          <dd>{active ? t("profiles.current") : "—"}</dd>
        </div>
      </dl>
      {profile.kind === "remote" && <section className="cb-subscription-status" aria-label={t("profiles.lastSuccess")}>
        <dl><div><dt>{t("profiles.lastSuccess")}</dt><dd>{status?.last_success?new Date(status.last_success*1000).toLocaleString():"—"}</dd></div><div><dt>{t(status?.retry?"profiles.retryAt":"profiles.nextUpdate")}</dt><dd>{!profile.update_interval_min?t("profiles.autoDisabled"):!status?.next_update?t("profiles.due"):status.next_update*1000<=Date.now()?t("profiles.due"):new Date(status.next_update*1000).toLocaleString()}</dd></div></dl>
        {status?.failure&&<div className="cb-workflow-callout"><h3>{t("profiles.previousConfig")}</h3><p className="cb-selectable">{status.failure}</p>{status.failure_at&&<small>{new Date(status.failure_at*1000).toLocaleString()}</small>}</div>}
        <p><b>{t("profiles.changes")}</b><span>{status?.changes?t("profiles.changeCounts",status.changes):t("profiles.changesUnknown")}</span></p>
      </section>}
      {profile.kind === "remote" && (
        <div className="cb-setting-row flex items-center justify-between">
          <div>
            <div className="text-sm">{t("profiles.remote")}</div>

          </div>
          <Select
            label={t("profiles.autoUpdate")}
            value={profile.update_interval_min}
            options={[
              { value: 0, label: t("profiles.autoOff") },
              ...[360, 720, 1440].map((min) => ({
                value: min,
                label: t("profiles.autoH", { n: min / 60 }),
              })),
            ]}
            onChange={(value) => onSetInterval(Number(value))}
          />
        </div>
      )}
      <div className="cb-profile-actions">
        <div>
          {traffic && traffic.total > 0 && (
            <>
              <ProgressRing
                size={44}
                value={pct}
                label={`${Math.round(pct)}%`}
              />
              <p className="text-[13px] tabular-nums">
                {fmtBytesLocal(used)} / {fmtBytesLocal(traffic.total)}
              </p>
            </>
          )}
        </div>
        <div>
          {profile.kind === "remote" && (
            <Button disabled={busy} onClick={onUpdate}>
              {t("profiles.update")}
            </Button>
          )}
          {!active && (
            <Button variant="primary"
              disabled={busy}
              onClick={onActivate}
            >
              {t("profiles.setActive")}
            </Button>
          )}
          <Button
            tone="danger"
            disabled={busy}
            onClick={onRemove}
          >
            {t("profiles.remove")}
          </Button>
        </div>
      </div>
    </section>
  );
}
