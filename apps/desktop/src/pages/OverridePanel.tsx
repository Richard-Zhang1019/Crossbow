import { useCallback, useEffect, useState } from "react";
import {
  ipc,
  type OverrideDef,
  type OverrideKind,
  type Profile,
} from "../ipc";
import { useT } from "../i18n";

/** 覆写面板：列表 + 新建 + 编辑器 + 绑定当前订阅。 */
export default function OverridePanel({
  profiles,
  activeId,
  onChanged,
}: {
  profiles: Profile[];
  activeId: string | null;
  onChanged: () => void;
}) {
  const t = useT();
  const [overrides, setOverrides] = useState<OverrideDef[]>([]);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [nameDraft, setNameDraft] = useState("");
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const active = profiles.find((p) => p.id === activeId);

  const refresh = useCallback(async () => {
    setOverrides(await ipc.listOverrides().catch(() => []));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const create = async (kind: OverrideKind) => {
    if (!newName.trim()) return;
    setCreating(true);
    try {
      const def = await ipc.createOverride(newName.trim(), kind);
      setNewName("");
      await refresh();
      setEditingId(def.id);
      setDraft(def.content);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  /**
   * 保存流程（校验先行）：
   * 1. 落库内容 → 2. 校验渲染 → 失败：⚠ 标注错误、内核不动
   *                     → 成功：清除错误并热重启应用
   */
  const save = async (id: string) => {
    try {
      await ipc.updateOverride(id, draft);
      await ipc.validateOverrides();
      await ipc.applyOverrides();
      setEditingId(null);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
      await refresh(); // 拉取 last_error 展示 ⚠
      onChanged();
    }
  };

  const commitRename = async (id: string, name: string) => {
    setRenamingId(null);
    if (!name.trim()) return;
    try {
      await ipc.updateOverride(id, undefined, name.trim());
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (id: string) => {
    try {
      await ipc.removeOverride(id);
      setEditingId(null);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleBind = async (oid: string, bound: boolean) => {
    if (!activeId) return;
    try {
      await ipc.toggleOverrideBinding(activeId, oid, bound);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const bound = (oid: string) =>
    active?.override_ids.includes(oid) ?? false;

  return (
    <section className="cb-card px-4 pb-4 pt-[14px]">
      <div className="flex items-center gap-2">
        <span className="cb-micro flex-1">{t("ov.title")}</span>
        <input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && newName.trim() && !creating) create("script");
          }}
          placeholder={t("ov.namePlaceholder")}
          className="cb-input cb-selectable w-44"
        />
        <button
          className="cb-btn acc py-1"
          disabled={creating || !newName.trim()}
          onClick={() => create("script")}
        >
          {creating ? "…" : t("ov.newScript")}
        </button>
        <button
          className="cb-btn py-1"
          disabled={creating || !newName.trim()}
          onClick={() => create("merge")}
        >
          {t("ov.newMerge")}
        </button>
      </div>

      {error && (
        <div
          className="mt-2 rounded-lg border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in srgb, var(--cb-bad) 40%, transparent)",
            color: "var(--cb-bad)",
          }}
        >
          {error}
        </div>
      )}

      {overrides.length === 0 ? (
        <div className="mt-2 text-xs" style={{ color: "var(--cb-text-dim)" }}>
          {t("ov.empty")}
        </div>
      ) : (
        <div className="mt-2 space-y-2">
          {overrides.map((o) => {
            const isBound = activeId ? bound(o.id) : false;
            return (
              <div
                key={o.id}
                className="rounded-[10px] border px-3 py-2.5"
                style={{
                  borderColor: o.enabled
                    ? "var(--cb-line-strong)"
                    : "var(--cb-line)",
                  background: "var(--cb-surface-2)",
                  opacity: o.enabled ? 1 : 0.55,
                }}
              >
                <div className="flex items-center gap-2">
                  <input
                    type="checkbox"
                    checked={o.enabled}
                    onChange={(e) =>
                      ipc
                        .setOverrideEnabled(o.id, e.target.checked)
                        .then(onChanged)
                    }
                    title={t("ov.enabled")}
                  />
                  {renamingId === o.id ? (
                    <input
                      autoFocus
                      value={nameDraft}
                      onChange={(e) => setNameDraft(e.target.value)}
                      onBlur={() => commitRename(o.id, nameDraft)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter")
                          commitRename(o.id, nameDraft);
                        if (e.key === "Escape") setRenamingId(null);
                      }}
                      className="cb-input py-0.5 text-[12.5px]"
                      style={{ width: 180 }}
                    />
                  ) : (
                    <>
                      <span
                        className="text-[12.5px] font-medium"
                        onDoubleClick={() => {
                          setNameDraft(o.name);
                          setRenamingId(o.id);
                        }}
                      >
                        {o.name}
                      </span>
                      <button
                        onClick={() => {
                          setNameDraft(o.name);
                          setRenamingId(o.id);
                        }}
                        title={t("profiles.rename")}
                        className="text-[11px] leading-none hover:opacity-80"
                        style={{ color: "var(--cb-faint)" }}
                      >
                        ✎
                      </button>
                    </>
                  )}
                  {o.last_error && (
                    <span
                      className="cursor-help text-[12px]"
                      style={{ color: "var(--cb-warn)" }}
                      title={o.last_error}
                    >
                      ⚠
                    </span>
                  )}
                  <span className="cb-badge gray">
                    {o.kind === "script" ? t("ov.kindScript") : t("ov.kindMerge")}
                  </span>
                  <div className="flex-1" />
                  <label
                    className="flex items-center gap-1 text-[11px]"
                    style={{ color: isBound ? "var(--cb-accent)" : "var(--cb-faint)" }}
                  >
                    <input
                      type="checkbox"
                      checked={isBound}
                      disabled={!activeId}
                      onChange={(e) => toggleBind(o.id, e.target.checked)}
                    />
                    {t("ov.bind")}
                  </label>
                  <button
                    className="cb-btn py-0.5"
                    onClick={() => {
                      setEditingId(editingId === o.id ? null : o.id);
                      setDraft(o.content);
                    }}
                  >
                    {t("ov.edit")}
                  </button>
                  <button
                    className="cb-btn py-0.5"
                    style={{ color: "var(--cb-bad)" }}
                    onClick={() => remove(o.id)}
                  >
                    ✕
                  </button>
                </div>

                {editingId === o.id && (
                  <div className="mt-2">
                    <textarea
                      value={draft}
                      onChange={(e) => setDraft(e.target.value)}
                      spellCheck={false}
                      className="cb-selectable w-full rounded-lg border p-2.5 font-mono text-[11.5px] leading-5 outline-none"
                      style={{
                        height: 180,
                        borderColor: "var(--cb-line-strong)",
                        background: "var(--cb-bg)",
                        color: "var(--cb-text)",
                      }}
                    />
                    <div className="mt-1.5 flex justify-end">
                      <button className="cb-btn acc py-1" onClick={() => save(o.id)}>
                        {t("ov.save")}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}
