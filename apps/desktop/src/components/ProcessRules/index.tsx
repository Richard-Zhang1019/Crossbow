import { useCallback, useEffect, useRef, useState } from "react";
import Select from "@/components/Select";
import Dialog from "@/components/Dialog";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/Button";
import { Input } from "@/components/Controls";
import { notify } from "@/notifications/service";
import { ipc, type ProcessRuleStatus } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import "./ProcessRules.css";

type Draft = { id: string | null; process: string; target: string };
export default function ProcessRules({
  presetProcess,
}: {
  presetProcess?: string;
}) {
  const t = useT();
  const [status, setStatus] = useState<ProcessRuleStatus | null>(null);
  const [readError, setReadError] = useState(false);
  const [editing, setEditing] = useState<Draft | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const alive = useRef(false),
    sequence = useRef(0),
    pending = useRef(false);
  const load = useCallback(async () => {
    const request = ++sequence.current;
    try {
      const next = await ipc.processRules();
      if (alive.current && sequence.current === request) {
        setStatus(next);
        setReadError(false);
      }
    } catch (error) {
      if (alive.current && sequence.current === request) {
        setReadError(true);
        notify.error(error, "process-rules-read");
      }
    }
  }, []);
  useEffect(() => {
    alive.current = true;
    void load();
    const off = [
      ipc.onProfileSwitched(() => void load()),
      ipc.onProfileUpdated(() => void load()),
    ];
    return () => {
      alive.current = false;
      sequence.current++;
      off.forEach((p) => void p.then((f) => f()));
    };
  }, [load]);
  useEffect(() => {
    if (presetProcess)
      setEditing({ id: null, process: presetProcess, target: "DIRECT" });
  }, [presetProcess]);
  const policy = (value: string) =>
    value === "DIRECT"
      ? t("pr.direct")
      : value === "REJECT"
        ? t("pr.reject")
        : value;
  const save = async () => {
    if (!editing || pending.current) return;
    const draft = editing,
      process = draft.process.trim();
    if (!process) {
      notify.error(t("pr.processRequired"), "process-rule-save");
      return;
    }
    pending.current = true;
    setBusy("save");
    try {
      const result = await notify.run(
        draft.id ? `process-rule-${draft.id}` : "process-rule-new",
        t("studio.processRules"),
        () => ipc.setProcessRule(draft.id, process, draft.target),
        t("pr.saved"),
      );
      if (alive.current && result.ok) {
        setEditing(null);
        await load();
      }
    } finally {
      pending.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const remove = async (id: string) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(id);
    try {
      const result = await notify.run(
        `process-rule-remove-${id}`,
        t("studio.processRules"),
        () => ipc.removeProcessRule(id),
        t("pr.removed"),
      );
      if (alive.current && result.ok) await load();
    } finally {
      pending.current = false;
      if (alive.current) setBusy(null);
    }
  };
  return (
    <section className="cb-process-rules" aria-label={t("studio.processRules")}>
      <div className="cb-process-toolbar">
        <span className="cb-process-count">
          {t("pr.list")}
          <span>{status?.rules.length ?? "—"}</span>
        </span>
        <Button
          variant="primary"
          disabled={!status || busy !== null || readError}
          onClick={() =>
            setEditing({ id: null, process: "", target: "DIRECT" })
          }
        >
          <span aria-hidden="true">＋</span>
          {t("pr.add")}
        </Button>
      </div>
      {readError ? (
        <div className="cb-process-empty">
          <p>{t("home.statePendingHint")}</p>
          <Button onClick={() => void load()}>{t("home.retryState")}</Button>
        </div>
      ) : !status ? (
        <p role="status" className="cb-control-note">
          {t("workflow.loading")}
        </p>
      ) : status.rules.length === 0 ? (
        <div className="cb-process-empty">
          <Icon name="sliders" size={26} />
          <h3>{t("pr.emptyTitle")}</h3>
          <p>{t("pr.empty")}</p>
        </div>
      ) : (
        <div
          className="cb-process-table"
          role="table"
          aria-label={t("pr.list")}
        >
          <div className="cb-process-columns" role="row">
            <span role="columnheader">{t("pr.processLabel")}</span>
            <span role="columnheader">{t("pr.targetLabel")}</span>
            <span role="columnheader">{t("pr.actions")}</span>
          </div>
          {status.rules.map((rule) => {
            const unavailable = !status.targets.includes(rule.target);
            return (
              <div key={rule.id} className="cb-process-row" role="row">
                <div className="cb-process-identity" role="cell">
                  <span className="cb-process-glyph">
                    <Icon name="cube" size={18} />
                  </span>
                  <strong title={rule.process}>{rule.process}</strong>
                </div>
                <div className="cb-process-route" role="cell">
                  <Icon name="arrow-right" />
                  <span className="cb-process-destination" title={rule.target}>
                    {policy(rule.target)}
                  </span>
                  {unavailable && (
                    <span className="cb-process-unavailable">
                      {t("pr.unavailable")}
                    </span>
                  )}
                </div>
                <div className="cb-process-actions" role="cell">
                  <Button
                    size="small"
                    disabled={busy !== null}
                    onClick={() =>
                      setEditing({
                        id: rule.id,
                        process: rule.process,
                        target: rule.target,
                      })
                    }
                  >
                    {t("pr.edit")}
                  </Button>
                  <Button
                    size="small"
                    tone="danger"
                    loading={busy === rule.id}
                    disabled={busy !== null}
                    onClick={() => void remove(rule.id)}
                  >
                    {t("pr.remove")}
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <p className="cb-process-footnote">{t("pr.footnote")}</p>
      {editing && (
        <Dialog
          title={t(editing.id ? "pr.editTitle" : "pr.add")}
          onClose={() => {
            if (!pending.current) setEditing(null);
          }}
        >
          <form
            className="cb-process-form"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <label>
              {t("pr.processLabel")}
              <Input
                autoFocus
                maxLength={128}
                disabled={busy !== null}
                value={editing.process}
                onChange={(e) =>
                  setEditing({ ...editing, process: e.target.value })
                }
                placeholder={t("pr.processPlaceholder")}
              />
            </label>
            <label>
              {t("pr.targetLabel")}
              <Select
                label={t("pr.targetLabel")}
                disabled={busy !== null}
                value={editing.target}
                options={(status?.targets ?? ["DIRECT", "REJECT"]).map(
                  (value) => ({ value, label: policy(value) }),
                )}
                onChange={(target) => setEditing({ ...editing, target })}
              />
            </label>
            <p className="cb-control-note">{t("pr.editorHint")}</p>
            <div className="cb-process-form-actions">
              <Button disabled={busy !== null} onClick={() => setEditing(null)}>
                {t("pr.cancel")}
              </Button>
              <Button
                type="submit"
                variant="primary"
                loading={busy === "save"}
                disabled={
                  busy !== null ||
                  !editing.process.trim() ||
                  !(status?.targets ?? []).includes(editing.target)
                }
              >
                {t("pr.save")}
              </Button>
            </div>
          </form>
        </Dialog>
      )}
    </section>
  );
}
