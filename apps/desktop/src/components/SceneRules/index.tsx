import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import Select from "@/components/Select";
import Dialog from "@/components/Dialog";
import { Input } from "@/components/Controls";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/Button";
import { subscriptionCleanup } from "@/lib/unlisten";
import { notify } from "@/notifications/service";
import { ipc, type ScenesStatus } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import "./SceneRules.css";

export default function SceneRules() {
  const t = useT();
  const [status, setStatus] = useState<ScenesStatus | null>(null);
  const [readError, setReadError] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  const [action, setAction] = useState<"direct" | "proxy">("direct");
  const request = useRef(0);
  const mounted = useRef(false);
  const operation = useRef(false);
  const addButton = useRef<HTMLButtonElement>(null);
  const load = useCallback(async () => {
    if (!mounted.current) return;
    const id = ++request.current;
    setLoading(true);
    try {
      const value = await ipc.networkScenes();
      if (!mounted.current || id !== request.current) return;
      setStatus(value);
      setReadError(false);
      void notify.resolve("scenes-read");
    } catch (error) {
      if (mounted.current && id === request.current) {
        setReadError(true);
        notify.error(error, "scenes-read");
      }
    } finally {
      if (mounted.current && id === request.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void load();
    const subscriptions = [
      listen("network://scene", () => void load()),
      ipc.onSysproxyStatus(() => void load()),
      ipc.onTunStatus(() => void load()),
    ].map(subscriptionCleanup);
    const onFocus = () => void load();
    window.addEventListener("focus", onFocus);
    return () => {
      mounted.current = false;
      ++request.current;
      subscriptions.forEach((cleanup) => cleanup());
      window.removeEventListener("focus", onFocus);
    };
  }, [load]);
  const requestLocation = useCallback(async () => {
    try {
      await ipc.requestSceneLocation();
      await load();
    } catch (error) {
      notify.error(error, "scene-location");
    }
  }, [load]);

  // 未发起过定位授权时自动触发一次系统框（挂载即用，无需找按钮）；
  // 允许/拒绝后状态改变不再触发；弹窗未决时状态值不变，effect 不重跑。
  useEffect(() => {
    if (mounted.current && !readError && status?.location === "not_determined") {
      void requestLocation();
    }
  }, [status?.location, readError, requestLocation]);

  useEffect(() => {
    if (status?.location !== "not_determined" || readError) return;
    let remaining = 60;
    const timer = window.setInterval(() => {
      void load();
      if (--remaining === 0) window.clearInterval(timer);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [status?.location, readError, load]);

  const mutate = async (ssid: string, next: "direct" | "proxy" | null) => {
    if (operation.current) return;
    operation.current = true;
    setBusy(true);
    try {
      const result = await notify.run(
        "scenes-write",
        t("settings.scenes.title"),
        () =>
          next === null
            ? ipc.removeNetworkScene(ssid)
            : ipc.setNetworkScene(ssid, next),
        undefined,
        { feedback: "error-only", titleKey: "settings.scenes.title" },
      );
      if (!mounted.current) return;
      if (result.ok) {
        setAdding(false);
        if (next === null) addButton.current?.focus();
      }
      await load();
    } finally {
      operation.current = false;
      if (mounted.current) setBusy(false);
    }
  };
  const unsupported = status?.supported === false;
  const disabled = busy || readError || !status || unsupported;
  const rules = Object.entries(status?.rules ?? {});
  const current = status?.currentSsid;
  const currentRule =
    current && Object.hasOwn(status?.rules ?? {}, current)
      ? status?.rules[current]
      : null;
  const active = status?.activeScene;
  const currentLabel = unsupported
    ? t("settings.scenes.unsupported")
    : (current ??
      t(
        status?.networkState === "disconnected"
          ? "settings.scenes.disconnected"
          : "settings.scenes.unavailable",
      ));
  const openAdd = () => {
    setName(current && !currentRule ? current : "");
    setAction("direct");
    setAdding(true);
  };
  const validName =
    name.trim().length > 0 &&
    new TextEncoder().encode(name).length <= 32 &&
    !Object.hasOwn(status?.rules ?? {}, name);
  const options = [
    { value: "direct", label: t("settings.scenes.ruleDirect") },
    { value: "proxy", label: t("settings.scenes.ruleProxy") },
  ];
  return (
    <section className="cb-scene-rules" aria-busy={loading}>
      <div className="cb-scene-intro">
        <p>{t("settings.scenes.hint")}</p>
        <div className="cb-scene-tools">
          <Button
            iconOnly
            size="small"
            aria-label={t("settings.scenes.refresh")}
            title={t("settings.scenes.refresh")}
            loading={loading}
            disabled={busy}
            onClick={() => void load()}
          >
            <Icon name="refresh" />
          </Button>
          <Button
            ref={addButton}
            size="small"
            disabled={disabled}
            onClick={openAdd}
          >
            {t("settings.scenes.addNetwork")}
          </Button>
        </div>
      </div>
      {readError ? (
        <div className="cb-scene-error" role="status">
          <span>{t("settings.scenes.loadFailed")}</span>
          <Button size="small" onClick={() => void load()}>
            {t("ov.retry")}
          </Button>
        </div>
      ) : (
        <div className="cb-scene-network">
          <div className="cb-scene-network-name">
            <span className="cb-scene-current-label">
              {t("settings.scenes.currentLabel")}
            </span>
            <strong title={current ?? undefined}>
              {loading && !status ? t("settings.scenes.loading") : currentLabel}
            </strong>
          </div>
          {current && !unsupported && (
            <span className="cb-scene-status">
              <i data-active={active?.[0] === current || undefined} />
              {active?.[0] === current
                ? t(
                    active[1] === "direct"
                      ? "settings.scenes.activeDirect"
                      : "settings.scenes.activeProxy",
                  )
                : t(
                    currentRule
                      ? "settings.scenes.pending"
                      : "settings.scenes.followGlobal",
                  )}
            </span>
          )}
        </div>
      )}
      {!current && !readError && status && !unsupported && (
        status.location === "not_determined" ? (
          <div className="cb-scene-access-note">
            <p>{t("settings.scenes.locationHint")}</p>
            <Button size="small" onClick={() => void requestLocation()}>
              {t("settings.scenes.requestLocation")}
            </Button>
          </div>
        ) : status.location === "denied" ? (
          <p className="cb-scene-access-note">
            {t("settings.scenes.locationDenied")}
          </p>
        ) : (
          <p className="cb-scene-access-note">{t(status.networkState === "disconnected" ? "settings.scenes.noWifi" : "settings.scenes.unavailable")}</p>
        )
      )}
      {rules.length > 0 ? (
        <div
          className="cb-scene-list"
          aria-label={t("settings.scenes.rulesLabel")}
        >
          <div className="cb-scene-table-head">
            <span>{t("settings.scenes.wifiName")}</span>
            <span>{t("settings.scenes.actionLabel")}</span>
            <span />
          </div>
          {rules.map(([ssid, value]) => (
            <div className="cb-scene-row" key={ssid}>
              <div className="cb-scene-identity">
                <strong title={ssid}>{ssid}</strong>
                {ssid === current && (
                  <span>{t("settings.scenes.connected")}</span>
                )}
              </div>
              <Select
                label={`${t("settings.scenes.actionLabel")} · ${ssid}`}
                value={value}
                disabled={disabled}
                options={[
                  ...options,
                  { value: "global", label: t("settings.scenes.followGlobal") },
                ]}
                onChange={(v) =>
                  void mutate(
                    ssid,
                    v === "global" ? null : v === "direct" ? "direct" : "proxy",
                  )
                }
              />
              <Button
                iconOnly
                size="small"
                tone="danger"
                disabled={disabled}
                aria-label={`${t("settings.scenes.remove")} ${ssid}`}
                title={t("settings.scenes.remove")}
                onClick={() => void mutate(ssid, null)}
              >
                <Icon name="trash" size={14} />
              </Button>
            </div>
          ))}
        </div>
      ) : (
        !loading &&
        !readError &&
        !unsupported && (
          <div className="cb-scene-empty">{t("settings.scenes.empty")}</div>
        )
      )}
      <p className="cb-scene-footnote">{t("settings.scenes.behavior")}</p>
      {adding && (
        <Dialog
          title={t("settings.scenes.addNetwork")}
          onClose={() => {
            if (!busy) setAdding(false);
          }}
        >
          <form
            className="cb-scene-form"
            onSubmit={(e) => {
              e.preventDefault();
              if (validName) void mutate(name, action);
            }}
          >
            <label htmlFor="scene-network-name">
              {t("settings.scenes.wifiName")}
            </label>
            <Input
              id="scene-network-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
              disabled={busy}
              maxLength={32}
              aria-describedby="scene-name-hint"
            />
            {name && !validName && (
              <p className="cb-control-note" id="scene-name-hint">
                {t(
                  Object.hasOwn(status?.rules ?? {}, name)
                    ? "settings.scenes.duplicate"
                    : "settings.scenes.invalidName",
                )}
              </p>
            )}
            <label>{t("settings.scenes.actionLabel")}</label>
            <Select
              label={t("settings.scenes.actionLabel")}
              value={action}
              onChange={(v) => setAction(v === "proxy" ? "proxy" : "direct")}
              options={options}
              disabled={busy}
            />
            <p className="cb-control-note">{t("settings.scenes.proxyHint")}</p>
            <div className="actions">
              <Button disabled={busy} onClick={() => setAdding(false)}>
                {t("ov.cancel")}
              </Button>
              <Button
                type="submit"
                variant="primary"
                disabled={!validName || busy}
                loading={busy}
              >
                {t("settings.scenes.save")}
              </Button>
            </div>
          </form>
        </Dialog>
      )}
    </section>
  );
}
