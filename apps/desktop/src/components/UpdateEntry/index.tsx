import { useEffect, useState } from "react";
import { Button } from "@/components/Button";
import { ProgressRing } from "@/components/ProgressRing";
import { Icon } from "@/components/Icon";
import { useT } from "@/i18n/i18n";
import { ipc, type UpdateState } from "@/services/ipc";

/** The toolbar stays subscribed across pages. Updates are applied from Settings. */
export function UpdateEntry({ onOpen }: { onOpen: () => void }) {
  const t = useT();
  const [update, setUpdate] = useState<UpdateState | null>(null);

  useEffect(() => {
    let active = true;
    let receivedEvent = false;
    const unlisten = ipc.onUpdateState((state) => {
      if (!active) return;
      receivedEvent = true;
      setUpdate(state);
    });
    void unlisten
      .then(() => ipc.updateStateGet())
      .then((state) => {
        if (active && !receivedEvent) setUpdate(state);
      })
      .catch(() => {});
    return () => {
      active = false;
      void unlisten.then((off) => off(), () => {});
    };
  }, []);

  if (!update) return null;
  const download = update.downloaded ? null : update.progress;
  const percent =
    download && download[1] > 0
      ? Math.min(
          100,
          Math.max(
            0,
            Math.floor((download[0] / download[1]) * 100),
          ),
        )
      : null;
  const title = update.downloaded
    ? t("home.updateReady")
    : download
      ? t("home.updateDownloading")
      : t("home.updateAvailable");
  const action = update.downloaded
    ? t("home.updateInstall")
    : download
      ? t("home.updateViewProgress")
      : t("home.updateGoBtn");

  const version = `v${update.version.replace(/^v/i, "")}`;
  const progress = percent === null ? null : `${percent}%`;
  const label = [title, version, progress, action].filter(Boolean).join(" · ");

  return (
    <Button context="toolbar" className="cb-update-entry" onClick={onOpen} aria-label={label} title={label}>
      <span className="cb-update-icon" aria-hidden="true">
        {download ? (
          <ProgressRing value={percent} label={title} size={16} />
        ) : (
          <Icon name={update.downloaded ? "check" : "download"} size={16} />
        )}
      </span>
      <span className="cb-update-title">{title}</span>
      <span className="cb-update-meta">{progress ?? version}</span>
    </Button>
  );
}
