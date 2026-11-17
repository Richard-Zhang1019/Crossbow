import { useEffect, useState } from "react";
import Dialog from "@/components/Dialog";
import { Button } from "@/components/Button";
import { Input } from "@/components/Controls";
import { Icon } from "@/components/Icon";
import { ipc } from "@/services/ipc";
import { useT } from "@/i18n/i18n";

/** #544：应用内反馈问题——表单生成预填 GitHub issue，浏览器打开后一键提交。
 *  自动附加的环境块只含白名单字段（版本/系统/内核/模式/端口），不含订阅、
 *  节点或凭据；描述由用户自行填写。无自建后端、无存储面。 */
export default function FeedbackDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [attachEnv, setAttachEnv] = useState(true);
  const [envPreview, setEnvPreview] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 环境块：打开时拉取一次（白名单字段；打开后再变化不追踪——反馈是时点快照）
  useEffect(() => {
    let alive = true;
    (async () => {
      const [version, core, bin, engine, proxy, tun] = await Promise.all([
        ipc.appVersion().catch(() => ""),
        ipc.coreStatus().catch(() => null),
        ipc.coreBinaryInfoFor((await ipc.getEngineConfig().catch(() => null))?.engine ?? "mihomo").catch(() => null),
        ipc.getEngineConfig().catch(() => null),
        ipc.sysproxyStatus().catch(() => null),
        ipc.tunStatus().catch(() => null),
      ]);
      if (!alive) return;
      const os = navigator.userAgent.includes("Macintosh") ? "macOS" : navigator.userAgent.includes("Windows") ? "Windows" : "Linux";
      const mode = [proxy?.enabled && "系统代理", tun?.enabled && "TUN"].filter(Boolean).join(" + ") || "未开启";
      setEnvPreview(
        [
          `- Crossbow: v${version || "unknown"}`,
          `- 系统: ${os}`,
          `- 内核: ${bin?.version || "unknown"} (${core?.state ?? "unknown"})`,
          `- 引擎: ${engine?.engine ?? "unknown"}`,
          `- 代理模式: ${mode}`,
          `- 混合端口: ${engine?.mixed_port ?? "?"}`,
        ].join("\n"),
      );
    })();
    return () => { alive = false; };
  }, []);

  const canSubmit = title.trim().length > 0 && description.trim().length > 0 && !busy;
  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      const envBlock = attachEnv ? `\n\n### 环境\n\n${envPreview}` : "";
      await ipc.openFeedbackIssue(title.trim(), `${description.trim()}${envBlock}`);
      onClose();
    } catch (e) {
      setError(String(e).replace(/^Error:\s*/, "").slice(0, 200));
      setBusy(false);
    }
  };

  return (
    <Dialog title={t("feedback.title")} onClose={onClose}>
      <p className="cb-control-note">{t("feedback.hint")}</p>
      <label htmlFor="feedback-title">{t("feedback.titleLabel")}</label>
      <Input
        id="feedback-title"
        value={title}
        maxLength={200}
        placeholder={t("feedback.titlePlaceholder")}
        onChange={(e) => setTitle(e.target.value)}
      />
      <label htmlFor="feedback-desc">{t("feedback.descLabel")}</label>
      <textarea
        id="feedback-desc"
        className="cb-input-inner"
        rows={5}
        maxLength={6000}
        placeholder={t("feedback.descPlaceholder")}
        value={description}
        onChange={(e) => setDescription(e.target.value)}
      />
      <button type="button" className="cb-feedback-env-toggle" onClick={() => setAttachEnv(v => !v)}>
        <Icon name={attachEnv ? "check" : "minus"} size={13} />
        {t("feedback.attachEnv")}
      </button>
      {attachEnv && (
        <pre className="cb-share-preview cb-selectable cb-mono" aria-label={t("feedback.attachEnv")}>{envPreview}</pre>
      )}
      {error && <p role="alert" style={{ color: "var(--cb-danger)" }}>{error}</p>}
      <div className="actions">
        <Button onClick={onClose}>{t("studio.close")}</Button>
        <Button variant="primary" disabled={!canSubmit} loading={busy} onClick={() => void submit()}>
          <Icon name="upload" />
          {t("feedback.submit")}
        </Button>
      </div>
    </Dialog>
  );
}
