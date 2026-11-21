import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/Button";
import { Input, Switch } from "@/components/Controls";
import Dialog from "@/components/Dialog";
import Select from "@/components/Select";
import PopoverPanel from "@/components/PopoverPanel";
import { subscriptionCleanup } from "@/lib/unlisten";
import { notify } from "@/notifications/service";
import { useT } from "@/i18n/i18n";
import { ipc, type RuleSetDraft, type RuleSetsStatus } from "@/services/ipc";
import "./RuleSets.css";
export default function RuleSets() {
  const t = useT();
  const [status, setStatus] = useState<RuleSetsStatus | null>(null);
  const [failed, setFailed] = useState(false);
  const [draft, setDraft] = useState<RuleSetDraft | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const alive = useRef(false),
    revision = useRef(0),
    pending = useRef(false);
  const load = useCallback(async () => {
    const request = ++revision.current;
    try {
      const next = await ipc.ruleSets();
      if (alive.current && request === revision.current) {
        setStatus(next);
        setFailed(false);
      }
    } catch (e) {
      if (alive.current && request === revision.current) {
        setFailed(true);
        notify.error(e, "rulesets-read");
      }
    }
  }, []);
  useEffect(() => {
    alive.current = true;
    void load();
    const off = [
      ipc.onRuleSetsUpdated(() => void load()),
      ipc.onProfileSwitched(() => void load()),
      ipc.onProfileUpdated(() => void load()),
    ].map(subscriptionCleanup);
    return () => {
      alive.current = false;
      ++revision.current;
      off.forEach((cleanup) => cleanup());
    };
  }, [load]);
  const run = async (key: string, task: () => Promise<void>, close = false) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(key);
    try {
      const result = await notify.run(
        `ruleset-${key}`,
        t("rs.title"),
        task,
        undefined,
        { feedback: "error-only" },
      );
      if (alive.current) {
        if (result.ok && close) setDraft(null);
        await load();
      }
    } finally {
      pending.current = false;
      if (alive.current) setBusy(null);
    }
  };
  const change = (
    id: string,
    action: "enable" | "disable" | "up" | "down" | "remove",
  ) => void run(`${action}-${id}`, () => ipc.changeRuleSet(id, action));
  const host = (url: string) => {
    try {
      return new URL(url).hostname;
    } catch {
      return "—";
    }
  };
  return (
    <section className="cb-rulesets" aria-label={t("rs.title")}>
      <div className="cb-rulesets-toolbar">
        <span>
          {t("rs.title")}{" "}
          <span className="cb-control-note">{status?.sets.length ?? "—"}</span>
        </span>
        <Button
          variant="primary"
          disabled={!status || failed || busy !== null}
          onClick={() =>
            setDraft({
              id: null,
              name: "",
              url: "",
              behavior: "domain",
              target:
                status?.targets.find((v) => v !== "DIRECT" && v !== "REJECT") ??
                "DIRECT",
              interval_secs: 86400,
            })
          }
        >
          {t("rs.add")}
        </Button>
      </div>
      {failed ? (
        <div className="cb-rulesets-empty">
          <p>{t("rs.loadFailed")}</p>
          <Button onClick={() => void load()}>{t("home.retryState")}</Button>
        </div>
      ) : !status ? (
        <p role="status">{t("workflow.loading")}</p>
      ) : status.sets.length === 0 ? (
        <div className="cb-rulesets-empty">
          <h3>{t("rs.emptyTitle")}</h3>
          <p>{t("rs.emptyHint")}</p>
        </div>
      ) : (
        <div
          className="cb-card cb-rulesets-table"
          role="table"
          aria-label={t("rs.title")}
        >
          <div className="cb-table-heading" role="rowgroup">
            <div className="cb-th cb-table-row cb-rulesets-columns" role="row">
              <span role="columnheader">{t("rs.name")}</span>
              <span role="columnheader">{t("pr.targetLabel")}</span>
              <span role="columnheader">{t("rs.cache")}</span>
              <span role="columnheader">{t("pr.actions")}</span>
            </div>
          </div>
          {status.sets.map((set, index) => (
            <div
              className="cb-table-row cb-rulesets-columns cb-rulesets-row"
              role="row"
              key={set.id}
            >
              <div className="cb-rulesets-identity" role="cell">
                <strong title={set.name}>{set.name}</strong>
                <span>
                  {host(set.url)} · {set.behavior}
                </span>
              </div>
              <div className="cb-rulesets-policy" role="cell">
                <span title={set.target}>
                  {set.target === "DIRECT"
                    ? t("pr.direct")
                    : set.target === "REJECT"
                      ? t("pr.reject")
                      : set.target}
                </span>
                {!status.targets.includes(set.target) && (
                  <small>{t("rs.unavailable")}</small>
                )}
              </div>
              <div className="cb-rulesets-cache" role="cell">
                <span>{t("rs.count", { n: set.count })}</span>
                <small>
                  {set.last_error
                    ? t("rs.cachedFailure")
                    : new Date(set.updated_at * 1000).toLocaleString(
                        undefined,
                        {
                          month: "numeric",
                          day: "numeric",
                          hour: "2-digit",
                          minute: "2-digit",
                        },
                      )}
                </small>
              </div>
              <div className="cb-rulesets-actions" role="cell">
                <Switch
                  aria-label={`${t("rs.enabled")} ${set.name}`}
                  checked={set.enabled}
                  disabled={busy !== null}
                  onChange={(v) => change(set.id, v ? "enable" : "disable")}
                />
                <PopoverPanel
                  label={`${t("pr.actions")} ${set.name}`}
                  trigger={
                    <Button size="small" disabled={busy !== null}>
                      {t("pr.actions")}
                    </Button>
                  }
                >
                  {(close) => (
                    <div className="cb-rulesets-menu">
                      <Button
                        onClick={() => {
                          close();
                          setDraft({
                            id: set.id,
                            name: set.name,
                            url: set.url,
                            behavior: set.behavior,
                            target: set.target,
                            interval_secs: set.interval_secs,
                          });
                        }}
                      >
                        {t("pr.edit")}
                      </Button>
                      <Button
                        onClick={() => {
                          close();
                          void run(`update-${set.id}`, () =>
                            ipc.updateRuleSet(set.id),
                          );
                        }}
                      >
                        {t("rs.update")}
                      </Button>
                      <Button
                        disabled={index === 0}
                        onClick={() => {
                          close();
                          change(set.id, "up");
                        }}
                      >
                        {t("rs.up")}
                      </Button>
                      <Button
                        disabled={index + 1 === status.sets.length}
                        onClick={() => {
                          close();
                          change(set.id, "down");
                        }}
                      >
                        {t("rs.down")}
                      </Button>
                      <Button
                        tone="danger"
                        onClick={() => {
                          close();
                          change(set.id, "remove");
                        }}
                      >
                        {t("pr.remove")}
                      </Button>
                    </div>
                  )}
                </PopoverPanel>
              </div>
            </div>
          ))}
        </div>
      )}
      <p className="cb-control-note">{t("rs.priority")}</p>
      {draft && (
        <Dialog
          title={t(draft.id ? "rs.edit" : "rs.add")}
          onClose={() => {
            if (!pending.current) setDraft(null);
          }}
        >
          <form
            className="cb-rulesets-form"
            onSubmit={(e) => {
              e.preventDefault();
              const value = draft;
              void run("save", () => ipc.saveRuleSet(value), true);
            }}
          >
            <label>
              {t("rs.name")}
              <Input
                autoFocus
                maxLength={80}
                value={draft.name}
                disabled={busy !== null}
                onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              />
            </label>
            <label>
              {t("rs.url")}
              <Input
                type="url"
                placeholder="https://…/rules.yaml"
                value={draft.url}
                disabled={busy !== null}
                onChange={(e) => setDraft({ ...draft, url: e.target.value })}
              />
            </label>
            <div className="cb-rulesets-form-pair">
              <label>
                {t("rs.behavior")}
                <Select
                  label={t("rs.behavior")}
                  value={draft.behavior}
                  disabled={busy !== null}
                  options={["domain", "ipcidr", "classical"].map((value) => ({
                    value,
                    label: value,
                  }))}
                  onChange={(v) =>
                    setDraft({
                      ...draft,
                      behavior: v as RuleSetDraft["behavior"],
                    })
                  }
                />
              </label>
              <label>
                {t("rs.interval")}
                <Select
                  label={t("rs.interval")}
                  value={String(draft.interval_secs)}
                  disabled={busy !== null}
                  options={[
                    ...new Set([
                      3600,
                      21600,
                      86400,
                      259200,
                      draft.interval_secs,
                    ]),
                  ]
                    .sort((a, b) => a - b)
                    .map((value) => ({
                      value: String(value),
                      label: t("rs.hours", { n: value / 3600 }),
                    }))}
                  onChange={(v) =>
                    setDraft({ ...draft, interval_secs: Number(v) })
                  }
                />
              </label>
            </div>
            <label>
              {t("pr.targetLabel")}
              <Select
                label={t("pr.targetLabel")}
                disabled={busy !== null}
                value={draft.target}
                options={(status?.targets ?? []).map((value) => ({
                  value,
                  label:
                    value === "DIRECT"
                      ? t("pr.direct")
                      : value === "REJECT"
                        ? t("pr.reject")
                        : value,
                }))}
                onChange={(target) => setDraft({ ...draft, target })}
              />
            </label>
            <p className="cb-control-note">{t("rs.formHint")}</p>
            <div className="cb-rulesets-form-actions">
              <Button disabled={busy !== null} onClick={() => setDraft(null)}>
                {t("pr.cancel")}
              </Button>
              <Button
                type="submit"
                variant="primary"
                loading={busy === "save"}
                disabled={
                  busy !== null ||
                  !draft.name.trim() ||
                  !draft.url.trim() ||
                  !status?.targets.includes(draft.target)
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
