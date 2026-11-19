import { useCallback, useEffect, useState } from "react";

import Select from "@/components/Select";
import { Button } from "@/components/Button";
import { Input } from "@/components/Controls";
import { notify } from "@/notifications/service";
import { ipc, type ProcessRuleStatus } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import "./ProcessRules.css";

/**
 * 按进程分流规则（#570）：进程名 → 策略目标（DIRECT/REJECT/策略组）。
 * 规则前插到订阅规则之前（渲染层注入），变更走内核热重载。
 * `presetProcess` 非空时打开新增表单并预填（连接页「为此进程创建规则」入口）。
 */
export default function ProcessRules({ presetProcess }: { presetProcess?: string }) {
  const t = useT();
  const [status, setStatus] = useState<ProcessRuleStatus | null>(null);
  const [readError, setReadError] = useState(false);
  const [editing, setEditing] = useState<{ id: string | null; process: string; target: string } | null>(
    presetProcess ? { id: null, process: presetProcess, target: "DIRECT" } : null,
  );

  const load = useCallback(async () => {
    try {
      setStatus(await ipc.processRules());
      setReadError(false);
    } catch (error) {
      setReadError(true);
      notify.error(error, "process-rules-read");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async (rule: { id: string | null; process: string; target: string }) => {
    const process = rule.process.trim();
    if (!process) {
      notify.error(t("pr.processRequired"), "process-rule-save");
      return;
    }
    const ok = await notify.run(
      rule.id ? `process-rule-${rule.id}` : "process-rule-new",
      t("studio.processRules"),
      () => ipc.setProcessRule(rule.id, process, rule.target),
      t("pr.saved"),
    );
    if (ok) {
      setEditing(null);
      void load();
    }
  };

  const remove = async (id: string) => {
    await notify.run(
      `process-rule-remove-${id}`,
      t("studio.processRules"),
      () => ipc.removeProcessRule(id),
      t("pr.removed"),
    );
    void load();
  };

  return (
    <div className="cb-process-rules">
      {readError ? (
        <p role="status">{t("home.statePendingHint")}</p>
      ) : (
        <>
          {editing ? (
            <form
              className="cb-process-form"
              onSubmit={(e) => {
                e.preventDefault();
                void save(editing);
              }}
            >
              <Input
                value={editing.process}
                onChange={(e) => setEditing({ ...editing, process: e.target.value })}
                placeholder={t("pr.processPlaceholder")}
                aria-label={t("pr.processLabel")}
              />
              <Select
                label={t("pr.targetLabel")}
                value={editing.target}
                options={(status?.targets ?? ["DIRECT"]).map((target) => ({
                  value: target,
                  label: target,
                }))}
                onChange={(v) => setEditing({ ...editing, target: v })}
              />
              <Button variant="primary" onClick={() => void save(editing)}>
                {t("pr.save")}
              </Button>
              <Button onClick={() => setEditing(null)}>{t("pr.cancel")}</Button>
            </form>
          ) : (
            <div className="cb-process-form">
              <Button
                variant="primary"
                onClick={() => setEditing({ id: null, process: "", target: "DIRECT" })}
              >
                {t("pr.add")}
              </Button>
            </div>
          )}
          {status && status.rules.length > 0 && (
            <div className="cb-process-list">
              {status.rules.map((rule) => (
                <div key={rule.id} className="cb-process-row">
                  <strong className="cb-process-name">{rule.process}</strong>
                  <span className="cb-process-target">{rule.target}</span>
                  <div className="cb-process-actions">
                    <Button
                      size="small"
                      onClick={() =>
                        setEditing({ id: rule.id, process: rule.process, target: rule.target })
                      }
                    >
                      {t("pr.edit")}
                    </Button>
                    <Button size="small" tone="danger" onClick={() => void remove(rule.id)}>
                      {t("pr.remove")}
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          )}
          {status && status.rules.length === 0 && !editing && (
            <p className="cb-control-note">{t("pr.empty")}</p>
          )}
        </>
      )}
    </div>
  );
}
