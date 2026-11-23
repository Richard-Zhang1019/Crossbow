import { useEffect, useId, useState } from "react";
import { IconAction } from "@/components/IconAction";
import { ControlRow, Switch } from "@/components/Controls";
import { useClipboardFeedback } from "@/hooks/useClipboardFeedback";
import { useT } from "@/i18n/i18n";
import { notify } from "@/notifications/service";
import { ipc, type LanAddress } from "@/services/ipc";
import "./LanSharing.css";

export function LanSharing({
  enabled,
  port,
  onChange,
}: {
  enabled: boolean;
  port?: number;
  onChange: (enabled: boolean) => void;
}) {
  const t = useT();
  const accessId = useId();
  const showAccess = enabled && port !== undefined;

  return (
    <div className="cb-lan-sharing">
      <ControlRow label={t("settings.lan")} hint={t("settings.lanHint")}>
        <Switch
          checked={enabled}
          onChange={onChange}
          aria-controls={showAccess ? accessId : undefined}
        />
      </ControlRow>
      {showAccess && <LanAccess id={accessId} port={port} />}
    </div>
  );
}

function LanAccess({ id, port }: { id: string; port: number }) {
  const t = useT();
  const [addresses, setAddresses] = useState<LanAddress[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  const { copied, copy } = useClipboardFeedback<string>("lan-address-copy");

  const refresh = () => {
    setAddresses(null);
    setFailed(false);
    setRevision((value) => value + 1);
  };

  useEffect(() => {
    let current = true;
    void ipc.lanAddresses().then(
      (result) => {
        if (!current) return;
        setAddresses(result);
        void notify.resolve("lan-addresses");
      },
      () => {
        if (!current) return;
        setFailed(true);
        void notify.error(t("settings.lanReadFailed"), "lan-addresses");
      },
    );
    return () => {
      current = false;
    };
  }, [revision]);

  useEffect(() => {
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, []);

  const loading = addresses === null && !failed;
  const hasAddresses = Boolean(addresses?.length);
  const accessNote = hasAddresses
    ? "settings.lanBlocked"
    : failed
      ? "settings.lanReadFailed"
      : loading
        ? "settings.lanLoading"
        : "settings.lanEmpty";

  return (
    <section id={id} className="cb-lan-access" aria-label={t("settings.lanAccess")}>
      <p>{t("settings.lanSetup")}</p>
      {hasAddresses && (
        <ul className="cb-lan-addresses">
          {addresses?.map(({ interface: name, address }) => {
            const endpoint = `${address}:${port}`;
            const key = `${name}/${endpoint}`;
            const justCopied = copied === key;
            return (
              <li key={key}>
                <div className="cb-lan-address">
                  <span className="cb-lan-endpoint cb-selectable">{endpoint}</span>
                <IconAction icon="copy" success={justCopied}
                  label={
                    justCopied
                      ? t("workflow.copied")
                      : t("settings.lanCopyAddress", { address: endpoint })
                  }
                  onClick={() => void copy(key, () => endpoint)}
                />
                  <span>{name}</span>
                </div>

              </li>
            );
          })}
        </ul>
      )}
      <div className="cb-lan-access-footer">
        <p className="cb-lan-access-note" role={hasAddresses ? undefined : "status"}>
          {t(accessNote)}
        </p>
        <IconAction presentation="button" icon="refresh" label={t("settings.lanRefresh")} loading={loading} onClick={refresh} />
      </div>
    </section>
  );
}
