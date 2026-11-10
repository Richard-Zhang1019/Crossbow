import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/Button";
import Dialog from "@/components/Dialog";
import { ipc, type ConfigSnapshot } from "@/services/ipc";
import { notify } from "@/notifications/service";
import { useT } from "@/i18n/i18n";
export default function ConfigRecovery({
  onClose,
  onRestored,
}: {
  onClose: () => void;
  onRestored: () => void;
}) {
  const t = useT(),
    alive = useRef(true);
  const [rows, setRows] = useState<ConfigSnapshot[]>([]),
    [phase, setPhase] = useState("loading"),
    [busyId, setBusyId] = useState<string | null>(null);
  const load = async () => {
    setPhase("loading");
    try {
      const rows = await ipc.configSnapshots();
      if (alive.current) {
        setRows(rows);
        setPhase("ready");
      }
    } catch (e) {
      if (alive.current) {
        setPhase("error");
        notify.error(e, "snapshot-list");
      }
    }
  };
  useEffect(() => {
    alive.current = true;
    void load();
    return () => {
      alive.current = false;
    };
  }, []);
  // 条 5（Ref #369）：恢复可逆（恢复前自动保留当前版本），不再二次确认——
  // 点「恢复」直接执行，进度与结果经通知反馈。
  const restore = async (row: ConfigSnapshot) => {
    if (busyId) return;
    setBusyId(row.id);
    const r = await notify.run(
      `snapshot-restore-${row.id}`,
      t("backup.title"),
      () => ipc.restoreConfigSnapshot(row.id),
      t("backup.done"),
    );
    if (r.ok) {
      onRestored(); // 恢复触发内核重载，父级状态刷新
      void load(); // 恢复前会自动生成新快照，列表就地刷新
    } else {
      setBusyId(null);
    }
  };
  return (
    <Dialog title={t("backup.title")} onClose={onClose}>
      <p className="cb-control-note">{t("backup.impact")}</p>
      {phase === "loading" ? (
        <p role="status">{t("workflow.loading")}</p>
      ) : phase === "error" ? (
        <Button onClick={() => void load()}>{t("home.retryState")}</Button>
      ) : rows.length === 0 ? (
        <p>{t("backup.empty")}</p>
      ) : (
        <div className="cb-recovery-list">
          {rows.map((row) => (
            <section key={row.id}>
              <div>
                <h3>{new Date(row.at * 1000).toLocaleString()}</h3>
                <p>{t("backup.counts", { profiles: row.profiles, overrides: row.overrides })}</p>
                <p className="cb-selectable">{row.active ?? "—"}</p>
              </div>
              <Button
                size="small"
                loading={busyId === row.id}
                disabled={busyId !== null}
                onClick={() => void restore(row)}
              >
                {t("backup.restore")}
              </Button>
            </section>
          ))}
        </div>
      )}
    </Dialog>
  );
}
