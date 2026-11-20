import { useEffect, useState } from "react";
import { Button } from "@/components/Button";
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

  return (
    <div className="cb-lan-sharing">
      <ControlRow label={t("settings.lan")} hint={t("settings.lanHint")}>
        <Switch checked={enabled} onChange={onChange} />
      </ControlRow>
      {enabled && port !== undefined && <LanAccess port={port} />}
    </div>
  );
}

function LanAccess({ port }: { port: number }) {
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

  return (
    <section className="cb-lan-access" aria-label={t("settings.lanAccess")}>
      <ControlRow label={t("settings.lanAccess")} hint={t("settings.lanSetup")}>
        <Button size="small" loading={loading} onClick={refresh}>
          {t("settings.lanRefresh")}
        </Button>
      </ControlRow>
      {addresses?.length ? (
        <>
          <ul className="cb-lan-addresses">
            {addresses.map(({ interface: name, address }) => {
              const endpoint = `${address}:${port}`;
              const key = `${name}/${endpoint}`;
              const justCopied = copied === key;
              return (
                <li key={key}>
                  <ControlRow label={name}>
                    <div className="cb-lan-address-readout">
                      <span className="cb-selectable">{endpoint}</span>
                      <Button
                        size="small"
                        aria-label={
                          justCopied
                            ? t("workflow.copied")
                            : t("settings.lanCopyAddress", { address: endpoint })
                        }
                        onClick={() => void copy(key, () => endpoint)}
                      >
                        {justCopied ? t("workflow.copied") : t("settings.lanCopy")}
                      </Button>
                    </div>
                  </ControlRow>
                </li>
              );
            })}
          </ul>
          <p className="cb-lan-access-note">{t("settings.lanBlocked")}</p>
        </>
      ) : (
        <p className="cb-lan-access-note" role="status">
          {t(failed ? "settings.lanReadFailed" : loading ? "settings.lanLoading" : "settings.lanEmpty")}
        </p>
      )}
    </section>
  );
}
