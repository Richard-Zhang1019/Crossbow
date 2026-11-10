import { useEffect, useRef, useState } from "react";
import PopoverPanel from "@/components/PopoverPanel";
import { Button, ButtonBase } from "@/components/Button";
import { Icon } from "@/components/Icon";
import { ipc, type Profile } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import { notify } from "@/notifications/service";
export default function ProfileSwitcher({
  currentName,
  onManage,
}: {
  currentName: string;
  onManage: () => void;
}) {
  const t = useT();
  const [profiles, setProfiles] = useState<Profile[]>([]),
    [active, setActive] = useState<string | null>(null),
    [busy, setBusy] = useState<string | null>(null),
    [failed, setFailed] = useState(false),
    [loading, setLoading] = useState(true);
  const flight = useRef(false),
    alive = useRef(true),
    revision = useRef(0);
  const load = async () => {
    const current = ++revision.current;
    setLoading(true);
    try {
      const [list, id] = await Promise.all([
        ipc.listProfiles(),
        ipc.activeProfileId(),
      ]);
      if (alive.current && current === revision.current) {
        setProfiles(list);
        setActive(id);
        setFailed(false);
        void notify.resolve("profile-switch-read");
      }
    } catch (e) {
      if (alive.current && current === revision.current) {
        setFailed(true);
        notify.error(e, "profile-switch-read");
      }
    } finally {
      if (alive.current && current === revision.current) setLoading(false);
    }
  };
  useEffect(() => {
    alive.current = true;
    void load();
    return () => {
      alive.current = false;
      revision.current++;
    };
  }, [currentName]);
  const choose = async (id: string, close: () => void) => {
    if (flight.current || id === active) return;
    flight.current = true;
    setBusy(id);
    const result = await notify.run(
      `profile-activate-${id}`,
      t("profiles.setActive"),
      () => ipc.setActiveProfile(id),
      undefined,
      { feedback: "error-only" },
    );
    if (result.ok && alive.current) {
      setActive(id);
      window.dispatchEvent(new CustomEvent("cb:profile-switched"));
      close();
    }
    if (alive.current) await load();
    flight.current = false;
    if (alive.current) setBusy(null);
  };
  const selected = profiles.find((p) => p.id === active);
  return (
    <PopoverPanel
      label={t("layout.quickProfiles")}
      onOpen={() => void load()}
      className="cb-profile-switcher"
      trigger={
        <Button
          context="toolbar"
          className="cb-source-pick"
          title={selected?.name || currentName || t("studio.sources")}
        >
          <span>{selected?.name || currentName || t("studio.sources")}</span>
          <Icon name="chevron" size={12} />
        </Button>
      }
    >
      {(close) => (
        <>
          <header>
            <b>{t("layout.quickProfiles")}</b>
            <span>{profiles.length}</span>
          </header>
          <div
            className="cb-profile-switch-list"
            onKeyDown={(e) => {
              if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key))
                return;
              const items = [
                ...e.currentTarget.querySelectorAll<HTMLButtonElement>(
                  "button:not(:disabled)",
                ),
              ];
              if (!items.length) return;
              const index = items.indexOf(
                document.activeElement as HTMLButtonElement,
              );
              const next =
                e.key === "Home"
                  ? 0
                  : e.key === "End"
                    ? items.length - 1
                    : e.key === "ArrowDown"
                      ? (index + 1) % items.length
                      : (index - 1 + items.length) % items.length;
              e.preventDefault();
              items[next]?.focus();
            }}
          >
            {loading && profiles.length === 0 ? (
              <p role="status">{t("workflow.loading")}</p>
            ) : failed ? (
              <Button onClick={() => void load()}>
                {t("home.retryState")}
              </Button>
            ) : profiles.length === 0 ? (
              <p>{t("profiles.empty")}</p>
            ) : (
              profiles.map((p) => (
                <ButtonBase
                  key={p.id}
                  className="cb-profile-switch-option"
                  aria-pressed={p.id === active}
                  disabled={!!busy || loading}
                  aria-busy={p.id===busy||undefined}
                  title={p.name}
                  onClick={() => void choose(p.id, close)}
                >
                  <Icon name="document" size={16} />
                  <span>
                    <b>{p.name}</b>
                    <small>
                      {t(
                        p.kind === "remote"
                          ? "profiles.remote"
                          : "profiles.local",
                      )}
                    </small>
                  </span>
                  {p.id === busy ? (
                    "…"
                  ) : p.id === active ? (
                    <Icon name="check" />
                  ) : null}
                </ButtonBase>
              ))
            )}
          </div>
          <footer>
            <Button
              onClick={() => {
                close();
                onManage();
              }}
            >
              {t("layout.manageProfiles")}
              <Icon name="arrow-right" size={13} />
            </Button>
          </footer>
        </>
      )}
    </PopoverPanel>
  );
}
