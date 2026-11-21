import { useCallback, useEffect, useRef, useState } from "react";
import SegmentedControl from "@/components/SegmentedControl";
import { Button } from "@/components/Button";
import { ControlRow, Input } from "@/components/Controls";
import { notify } from "@/notifications/service";
import { ipc, type DnsMode, type DnsSettings } from "@/services/ipc";
import { useT } from "@/i18n/i18n";

const toList = (value: string) => value.split(/[\n,，]/).map(item => item.trim()).filter(Boolean);
const join = (list: string[]) => list.join(", ");
const signature = (dns: DnsSettings) => JSON.stringify([
  dns.mode, dns.proxy_nameservers, dns.direct_nameservers, dns.bootstrap_nameservers,
]);
const equal = (a: DnsSettings, b: DnsSettings) => signature(a) === signature(b);

export function DnsSettingsCard({ engine }: { engine: "mihomo" | "singbox" | undefined }) {
  const t = useT();
  const [mode, setMode] = useState<DnsMode>("inherit");
  const [proxy, setProxy] = useState("");
  const [direct, setDirect] = useState("");
  const [bootstrap, setBootstrap] = useState("");
  const [saved, setSaved] = useState<DnsSettings | null>(null);
  const [failed, setFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const mounted = useRef(false);
  const busy = useRef(false);
  const generation = useRef(0);
  const load = useCallback(async () => {
    const request = ++generation.current;
    setFailed(false);
    try {
      const dns = await ipc.getDnsSettings();
      if (!mounted.current || generation.current !== request) return;
      setMode(dns.mode);
      setProxy(join(dns.proxy_nameservers));
      setDirect(join(dns.direct_nameservers));
      setBootstrap(join(dns.bootstrap_nameservers));
      setSaved(dns);
      void notify.resolve("dns-settings-load");
    } catch (error) {
      if (!mounted.current || generation.current !== request) return;
      setFailed(true);
      notify.error(error, "dns-settings-load");
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; generation.current++; };
  }, [load]);

  const draft: DnsSettings = {
    mode,
    proxy_nameservers: toList(proxy),
    direct_nameservers: toList(direct),
    bootstrap_nameservers: toList(bootstrap),
  };
  const dirty = saved !== null && !equal(saved, draft);
  const save = async () => {
    if (busy.current || !dirty) return;
    busy.current = true;
    setSaving(true);
    try {
      // One backend transaction owns validation, runtime apply, persistence and rollback.
      await ipc.setDnsSettings(draft);
      if (mounted.current) setSaved(draft);
      notify.success(t("dns.saved"), "dns-settings");
    } catch (error) {
      // The committed settings remain unchanged; keep this draft available to correct/retry.
      notify.error(error, "dns-settings");
    } finally {
      busy.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  return <div className="cb-dns-card">
    <fieldset className="cb-dns-fields" disabled={!saved || !engine || saving} aria-busy={saving || (!saved && !failed)}>
      <ControlRow label={t("dns.mode")} hint={t("dns.modeHint")}>
        <SegmentedControl label={t("dns.mode")} value={mode} onChange={setMode} options={[
          { value: "inherit", label: t("dns.modeInherit") },
          { value: "custom", label: t("dns.modeCustom") },
        ]} />
      </ControlRow>
      {mode === "custom" && <>
        <ControlRow label={t("dns.proxy")} hint={t("dns.proxyHint")}>
          <Input value={proxy} onChange={e => setProxy(e.target.value)} placeholder={t("dns.upstreamPlaceholder")} spellCheck={false} />
        </ControlRow>
        <ControlRow label={t("dns.direct")} hint={t(engine === "singbox" ? "dns.directSingboxHint" : "dns.directHint")}>
          <Input value={direct} onChange={e => setDirect(e.target.value)} placeholder={t("dns.upstreamPlaceholder")} spellCheck={false} />
        </ControlRow>
        <ControlRow label={t("dns.bootstrap")} hint={t(engine === "singbox" ? "dns.bootstrapSingboxHint" : "dns.bootstrapHint")}>
          <Input value={bootstrap} onChange={e => setBootstrap(e.target.value)} placeholder={t("dns.upstreamPlaceholder")} disabled={engine === "singbox"} spellCheck={false} />
        </ControlRow>
        <p className="cb-setting-hint">{t(engine === "singbox" ? "dns.noteSingbox" : "dns.noteMihomo")}</p>
        <p className="cb-setting-hint">{t("dns.noteIpv6")}</p>
      </>}
      {saved && <div className="cb-dns-actions">
        <Button size="small" variant="primary" loading={saving} disabled={!dirty} onClick={() => void save()}>{t("dns.save")}</Button>
      </div>}
    </fieldset>
    {failed && <div className="cb-dns-actions" role="status">
      <span className="cb-setting-hint">{t("dns.loadFailed")}</span>
      <Button size="small" onClick={() => void load()}>{t("dns.retry")}</Button>
    </div>}
  </div>;
}
