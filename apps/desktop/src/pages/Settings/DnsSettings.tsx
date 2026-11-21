import { useCallback, useEffect, useState } from "react";
import SegmentedControl from "@/components/SegmentedControl";
import { Button } from "@/components/Button";
import { ControlRow, Input } from "@/components/Controls";
import { notify } from "@/notifications/service";
import { ipc, type DnsMode, type DnsSettings } from "@/services/ipc";
import { useT } from "@/i18n/i18n";

/** 逗号/空格/换行分隔的上游列表 → 数组。 */
const toList = (value: string) =>
  value
    .split(/[\n,，]/)
    .map((item) => item.trim())
    .filter(Boolean);

const join = (list: string[]) => list.join(", ");

/** #598 DNS 设置卡片：沿用订阅（默认零变更）/ 受控自定义双模式。 */
export function DnsSettingsCard() {
  const t = useT();
  const [mode, setMode] = useState<DnsMode>("inherit");
  const [proxy, setProxy] = useState("");
  const [direct, setDirect] = useState("");
  const [bootstrap, setBootstrap] = useState("");
  const [engine, setEngine] = useState<"mihomo" | "singbox">("mihomo");
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async (silent = false) => {
    try {
      const [dns, engineConfig] = await Promise.all([
        ipc.getDnsSettings(),
        ipc.getEngineConfig(),
      ]);
      setMode(dns.mode);
      setProxy(join(dns.proxy_nameservers));
      setDirect(join(dns.direct_nameservers));
      setBootstrap(join(dns.bootstrap_nameservers));
      setEngine(engineConfig.engine);
      setFailed(false);
      setLoaded(true);
    } catch (error) {
      setFailed(true);
      if (!silent) notify.error(error, "dns-settings-load");
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    if (saving) return;
    setSaving(true);
    try {
      const settings: DnsSettings = {
        mode,
        proxy_nameservers: toList(proxy),
        direct_nameservers: toList(direct),
        bootstrap_nameservers: toList(bootstrap),
      };
      await ipc.setDnsSettings(settings);
      // 配置事务：渲染 + 内核热重载（渲染失败保留旧配置，回滚显示）
      await ipc.applyOverrides();
      notify.success(t("dns.saved"), "dns-settings");
      await load(true);
    } catch (error) {
      notify.error(error, "dns-settings");
      await load(true); // 失败回滚到真实状态
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="cb-dns-card">
      <ControlRow label={t("dns.mode")} hint={t("dns.modeHint")}>
        <SegmentedControl
          label={t("dns.mode")}
          value={mode}
          onChange={(value) => {
            if (value === "inherit" || value === "custom") setMode(value);
          }}
          options={[
            { value: "inherit", label: t("dns.modeInherit") },
            { value: "custom", label: t("dns.modeCustom") },
          ]}
        />
      </ControlRow>
      {mode === "custom" && (
        <>
          <ControlRow label={t("dns.proxy")} hint={t("dns.proxyHint")}>
            <Input
              value={proxy}
              onChange={(e) => setProxy(e.target.value)}
              placeholder={t("dns.upstreamPlaceholder")}
              spellCheck={false}
            />
          </ControlRow>
          <ControlRow label={t("dns.direct")} hint={t("dns.directHint")}>
            <Input
              value={direct}
              onChange={(e) => setDirect(e.target.value)}
              placeholder={t("dns.upstreamPlaceholder")}
              spellCheck={false}
            />
          </ControlRow>
          <ControlRow
            label={t("dns.bootstrap")}
            hint={
              engine === "singbox"
                ? t("dns.bootstrapSingboxHint")
                : t("dns.bootstrapHint")
            }
          >
            <Input
              value={bootstrap}
              onChange={(e) => setBootstrap(e.target.value)}
              placeholder={t("dns.upstreamPlaceholder")}
              disabled={engine === "singbox"}
              spellCheck={false}
            />
          </ControlRow>
        </>
      )}
      <ul className="cb-dns-notes">
        <li>{t("dns.noteProxy")}</li>
        <li>{t("dns.noteDirect")}</li>
        <li>{t("dns.noteBootstrap")}</li>
        <li>{t("dns.noteHosts")}</li>
        <li>{t("dns.noteIpv6")}</li>
        {engine === "singbox" && <li>{t("dns.noteSingbox")}</li>}
      </ul>
      {failed && !loaded && (
        <p role="status">{t("dns.loadFailed")}</p>
      )}
      {loaded && (
        <Button
          size="small"
          variant="primary"
          loading={saving}
          onClick={() => void save()}
        >
          {t("dns.save")}
        </Button>
      )}
      {failed && loaded && (
        <Button size="small" onClick={() => void load(false)}>
          {t("dns.retry")}
        </Button>
      )}
    </div>
  );
}
