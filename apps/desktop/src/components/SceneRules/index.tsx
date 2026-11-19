import { useCallback, useEffect, useState } from "react";

import Select from "@/components/Select";
import { Button } from "@/components/Button";
import { notify } from "@/notifications/service";
import { ipc, type ScenesStatus } from "@/services/ipc";
import { useT } from "@/i18n/i18n";
import "./SceneRules.css";

/**
 * 网络场景（#566）：SSID → 进入该网络时的接管动作（直连/代理）。
 * 数据自包含：挂载加载 + 接管状态事件后刷新 + 操作后就地重读。
 * SSID 读不到（无 Wi-Fi / 定位权限缺失）时功能保持可见但给出授权提示——
 * 后端对读不到 SSID 一律不干预，不会误切换。
 */
export default function SceneRules() {
  const t = useT();
  const [status, setStatus] = useState<ScenesStatus | null>(null);
  const [readError, setReadError] = useState(false);

  const load = useCallback(async () => {
    try {
      setStatus(await ipc.networkScenes());
      setReadError(false);
    } catch (error) {
      setReadError(true);
      notify.error(error, "scenes-read");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 接管状态变化（含场景引擎自身的切换）后刷新生效状态显示。
  useEffect(() => {
    const un = ipc.onSysproxyStatus(() => void load());
    return () => void un.then((f) => f());
  }, [load]);

  const setRule = async (ssid: string, action: "direct" | "proxy") => {
    await notify.run(
      `scene-set-${ssid}`,
      t("settings.scenes.title"),
      () => ipc.setNetworkScene(ssid, action),
      t("settings.scenes.saved"),
    );
    void load();
  };

  const removeRule = async (ssid: string) => {
    await notify.run(
      `scene-remove-${ssid}`,
      t("settings.scenes.title"),
      () => ipc.removeNetworkScene(ssid),
      t("settings.scenes.removed"),
    );
    void load();
  };

  const rules = status ? Object.entries(status.rules) : [];
  const currentInRules =
    status?.currentSsid != null && status.rules[status.currentSsid] != null;

  return (
    <div className="cb-scene-rules">
      <p className="cb-control-note">{t("settings.scenes.hint")}</p>
      {readError ? (
        <p role="status">{t("home.statePendingHint")}</p>
      ) : status == null ? null : status.currentSsid == null ? (
        <p className="cb-control-note">{t("settings.scenes.noWifi")}</p>
      ) : (
        <div className="cb-scene-current">
          <span>{t("settings.scenes.current")}</span>
          <strong>{status.currentSsid}</strong>
          <span className="cb-scene-active">
            {status.activeScene?.[0] === status.currentSsid
              ? t(
                  status.activeScene?.[1] === "direct"
                    ? "settings.scenes.activeDirect"
                    : "settings.scenes.activeProxy",
                )
              : t("settings.scenes.notInScene")}
          </span>
        </div>
      )}
      {status?.currentSsid != null && !currentInRules && (
        <div className="cb-scene-actions">
          <Button
            size="small"
            onClick={() => void setRule(status.currentSsid as string, "proxy")}
          >
            {t("settings.scenes.add")}
          </Button>
        </div>
      )}
      {rules.length > 0 && (
        <div className="cb-scene-list">
          {rules.map(([ssid, action]) => (
            <div key={ssid} className="cb-scene-row">
              <strong className="cb-scene-ssid">{ssid}</strong>
              <Select
                label={t("settings.scenes.actionLabel")}
                value={action === "direct" ? "direct" : "proxy"}
                options={[
                  { value: "proxy", label: t("settings.scenes.ruleProxy") },
                  { value: "direct", label: t("settings.scenes.ruleDirect") },
                ]}
                onChange={async (v) => {
                  await setRule(ssid, v === "direct" ? "direct" : "proxy");
                }}
              />
              <Button size="small" onClick={() => void removeRule(ssid)}>
                {t("settings.scenes.remove")}
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
