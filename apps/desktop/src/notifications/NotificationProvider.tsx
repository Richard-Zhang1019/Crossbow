import { ButtonBase, Button } from "../components/Button";
import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { currentLang, initLang, useT } from "../i18n";
import { ProgressRing } from "../components/ProgressRing";
import { Icon } from "../components/Icon";
import { ipc } from "../ipc";
import {
  notificationText as text,
  type Notification,
  type NotificationSnapshot,
} from "./service";
import "./notifications.css";

export const isNotificationWindow = new URLSearchParams(location.search).has(
  "notifications",
);
const initial: NotificationSnapshot = {
  revision: -1,
  active: [],
  history: [],
  navigateTo: null,
};
const Context = createContext({
  ...initial,
  historyOpen: false,
  setHistoryOpen: (_open: boolean) => {},
});

export function NotificationProvider({ children }: { children?: ReactNode }) {
  const [snapshot, setSnapshot] = useState(initial);
  const [historyOpen, setHistoryOpen] = useState(false);
  useEffect(() => {
    let disposed = false;
    let off: (() => void) | undefined;
    const receive = (next: NotificationSnapshot) => {
      if (disposed) return;
      setSnapshot((previous) =>
        next.revision > previous.revision ? next : previous,
      );
    };
    // Listen before requesting the snapshot to avoid dropping a background
    // result while the main webview is being recreated.
    void listen<NotificationSnapshot>("app://notifications", (event) =>
      receive(event.payload),
    )
      .then(async (unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        off = unlisten;
        receive(await invoke<NotificationSnapshot>("notification_snapshot"));
      })
      .catch((error) => console.error("notification initialization", error));
    return () => {
      disposed = true;
      off?.();
    };
  }, []);
  return (
    <Context.Provider value={{ ...snapshot, historyOpen, setHistoryOpen }}>
      {children}
      <NotificationViewport />
    </Context.Provider>
  );
}

function NotificationViewport() {
  const snapshot = useContext(Context);
  const stack = useRef<HTMLDivElement>(null);
  const [leaving, setLeaving] = useState<Notification[]>([]);
  const previous = useRef<Notification[]>([]);
  const leaveTimers = useRef(new Map<string, number>());
  useT(); // Refresh the labels when the application's language changes.
  useEffect(() => {
    if (snapshot.historyOpen) {
      // Only dismiss presentation; opening history never cancels a task or
      // resolves a problem. No backend "center open" flag can outlive the view.
      snapshot.active.forEach(
        (notice) =>
          void invoke("notification_dismiss", { id: notice.id }).catch(
            console.error,
          ),
      );
      previous.current = [];
      setLeaving([]);
      return;
    }
    const removed = previous.current.filter(
      (n) => !snapshot.active.some((a) => a.id === n.id),
    );
    previous.current = snapshot.active;
    for (const notice of snapshot.active) {
      window.clearTimeout(leaveTimers.current.get(notice.id));
      leaveTimers.current.delete(notice.id);
    }
    setLeaving((prev) => [
      ...prev.filter(
        (n) =>
          !snapshot.active.some((a) => a.id === n.id) &&
          !removed.some((r) => r.id === n.id),
      ),
      ...removed,
    ]);
    for (const notice of removed) {
      window.clearTimeout(leaveTimers.current.get(notice.id));
      leaveTimers.current.set(
        notice.id,
        window.setTimeout(() => {
          setLeaving((prev) => prev.filter((n) => n.id !== notice.id));
          leaveTimers.current.delete(notice.id);
        }, 180),
      );
    }
  }, [snapshot.active, snapshot.historyOpen]);
  useEffect(
    () => () => {
      leaveTimers.current.forEach(window.clearTimeout);
    },
    [],
  );
  useEffect(() => {
    if (!isNotificationWindow || !stack.current) return;
    const observer = new ResizeObserver(() => {
      void invoke("notification_resize", {
        height: stack.current!.scrollHeight + 32,
      }).catch(console.error);
    });
    observer.observe(stack.current);
    return () => observer.disconnect();
  }, [snapshot.active.length, leaving.length]);
  if (snapshot.historyOpen || (!snapshot.active.length && !leaving.length))
    return null;
  return (
    <div
      className={`cb-notification-viewport ${isNotificationWindow ? "standalone" : ""}`}
      ref={stack}
      aria-label={text("history")}
    >
      {[...snapshot.active, ...leaving].map((notice) => {
        const isLeaving = leaving.some((l) => l.id === notice.id);
        return (
          <NotificationCard
            key={notice.id}
            notice={notice}
            inert={isLeaving}
            leaving={isLeaving}
          />
        );
      })}
    </div>
  );
}

function StatusIcon({ kind }: { kind: Notification["kind"] }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="cb-notification-icon"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="9" />
      {kind === "success" ? (
        <path d="m8 12 3 3 5-6" />
      ) : kind === "error" ? (
        <path d="m9 9 6 6m0-6-6 6" />
      ) : (
        <path d="M12 8v5m0 3h.01" />
      )}
    </svg>
  );
}
function NotificationCard({
  notice,
  history = false,
  inert = false,
  leaving = false,
}: {
  notice: Notification;
  history?: boolean;
  inert?: boolean;
  leaving?: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const [copyState, setCopyState] = useState("");
  const hover = useRef(false);
  const focused = useRef(false);
  const pause = () => {
    if (!history && !inert)
      void invoke("notification_pause", {
        id: notice.id,
        paused: hover.current || focused.current || expanded,
      }).catch(console.error);
  };
  useEffect(pause, [expanded, history]);
  useEffect(
    () => () => {
      if (!history)
        void invoke("notification_pause", {
          id: notice.id,
          paused: false,
        }).catch(console.error);
    },
    [notice.id, history],
  );
  const t = useT();
  const title = notice.titleKey
    ? t(notice.titleKey)
    : notice.title || text(notice.code || notice.kind);
  const description = notice.description
    ? notice.code === "core.install"
      ? text(`stage.${notice.description}`)
      : notice.description
    : notice.kind === "error"
      ? text("failedHint")
      : notice.code === "core.safeMode"
        ? text("safeMode.hint")
        : notice.code === "tun.disabled"
          ? text("tun.disabled.hint")
          : "";
  const detailsId = `notice-details-${notice.id}`;
  const progressing = notice.kind === "progress";
  const percent =
    notice.progress !== null && Number.isFinite(notice.progress)
      ? Math.round(Math.min(100, Math.max(0, notice.progress)))
      : null;
  const progressDescription =
    description ||
    (notice.code === "update.download"
      ? text("update.connecting")
      : notice.code === "update.verifying"
        ? text("update.verifying.hint")
        : "");
  return (
    <article
      className={`cb-notification ${notice.kind} ${history ? "history" : ""} ${leaving ? "cb-notification-leaving" : ""}`}
      aria-hidden={inert || undefined}
      onMouseEnter={() => {
        hover.current = true;
        pause();
      }}
      onMouseLeave={() => {
        hover.current = false;
        pause();
      }}
      onFocus={() => {
        focused.current = true;
        pause();
      }}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget)) {
          focused.current = false;
          pause();
        }
      }}
    >
      {progressing ? (
        <ProgressRing value={notice.progress} label={title} />
      ) : (
        <StatusIcon kind={notice.kind} />
      )}
      <div className="cb-notification-body">
        <div
          role={
            history || inert
              ? undefined
              : notice.kind === "error"
                ? "alert"
                : "status"
          }
          aria-atomic="true"
        >
          <div className="cb-notification-heading">
            <div className="cb-notification-title">
              {title}
              {notice.count > 1 && (
                <span className="cb-notification-count">×{notice.count}</span>
              )}
            </div>
          </div>
          {!progressing && description && (
            <p className="cb-selectable">{description}</p>
          )}
        </div>
        {progressing && progressDescription && (
          <p className="cb-selectable cb-notification-transfer">
            {progressDescription}
          </p>
        )}
        <div className="cb-notification-actions">
          {notice.details && (
            <ButtonBase
              aria-expanded={expanded}
              aria-controls={detailsId}
              onClick={() => setExpanded(!expanded)}
              tabIndex={inert ? -1 : undefined}
            >
              {text("details")}
            </ButtonBase>
          )}
          {notice.actions.map((action) => (
            <ButtonBase
              key={action}
              tabIndex={inert ? -1 : undefined}
              onClick={() =>
                void invoke("notification_action", { action }).catch(
                  console.error,
                )
              }
            >
              {text(action)}
            </ButtonBase>
          ))}
        </div>
        {expanded && (
          <div id={detailsId} className="cb-notification-details">
            <pre className="cb-selectable">{notice.details}</pre>
            <ButtonBase
              onClick={async () => {
                try {
                  await navigator.clipboard.writeText(notice.details);
                  setCopyState(text("copied"));
                } catch {
                  setCopyState(text("copyFailed"));
                }
              }}
            >
              {text("copy")}
            </ButtonBase>
            <span role="status">{copyState}</span>
          </div>
        )}
      </div>
      {progressing && percent !== null && (
        <span className="cb-notification-percent" aria-hidden="true">
          {percent}
          <span>%</span>
        </span>
      )}
      {!history && (
        <ButtonBase
          className="cb-notification-close"
          aria-label={text("close")}
          tabIndex={inert ? -1 : undefined}
          onClick={() =>
            void invoke("notification_dismiss", { id: notice.id }).catch(
              console.error,
            )
          }
        >
          ×
        </ButtonBase>
      )}
      {history && (
        <time>
          {new Date(notice.createdAt).toLocaleTimeString(
            currentLang() === "en" ? "en-US" : "zh-CN",
            { hour: "2-digit", minute: "2-digit" },
          )}
        </time>
      )}
    </article>
  );
}

export function isPending(notice: Notification) {
  return (
    (notice.kind === "error" || notice.kind === "warning") && !notice.resolvedAt
  );
}
function HistoryRow({
  notice,
  onAction,
}: {
  notice: Notification;
  onAction: () => void;
}) {
  const t = useT();
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState("");
  const resolved = !!notice.resolvedAt;
  const kind = resolved ? "success" : notice.kind;
  const title = notice.titleKey
    ? t(notice.titleKey)
    : notice.title || text(notice.code || notice.kind);
  const recoveredTitle =
    resolved && !/recovered|resolved/.test(notice.code)
      ? `${title} · ${text("resolved")}`
      : title;
  const description = resolved
    ? text("resolvedHint")
    : notice.description
      ? notice.code === "core.install"
        ? text(`stage.${notice.description}`)
        : notice.description
      : notice.code === "core.safeMode"
        ? text("safeMode.hint")
        : notice.code === "tun.disabled"
          ? text("tun.disabled.hint")
          : kind === "error"
            ? text("failedHint")
            : "";
  const read = () =>
    void invoke("notification_mark_read", { id: notice.id }).catch(
      console.error,
    );
  const act = (action: string) => {
    read();
    onAction();
    void invoke("notification_action", { action }).catch(console.error);
  };
  const actions = resolved ? [] : notice.actions;
  const detailsId = `history-details-${notice.id}`;
  return (
    <article
      className={`cb-history-row ${kind} ${!notice.readAt && kind !== "progress" ? "unread" : ""}`}
    >
      <div className="cb-history-status">
        {kind === "progress" ? (
          <ProgressRing value={notice.progress} label={title} size={18} />
        ) : (
          <StatusIcon kind={kind} />
        )}
      </div>
      <div className="cb-history-content">
        <div className="cb-history-heading">
          <h3>
            {recoveredTitle}
            {notice.count > 1 && (
              <span className="cb-history-count">×{notice.count}</span>
            )}
          </h3>
          <time
            dateTime={new Date(
              notice.updatedAt || notice.createdAt,
            ).toISOString()}
          >
            {new Date(notice.updatedAt || notice.createdAt).toLocaleTimeString(
              currentLang() === "en" ? "en-GB" : "zh-CN",
              { hour: "2-digit", minute: "2-digit" },
            )}
          </time>
        </div>
        {description && <p>{description}</p>}
        {kind === "progress" && notice.progress !== null && (
          <p className="cb-history-progress">{Math.round(notice.progress)}%</p>
        )}
        {(actions.length > 0 || notice.details) && (
          <div className="cb-history-actions">
            {actions[0] && (
              <ButtonBase onClick={() => act(actions[0])}>
                {text(actions[0])}
                <span aria-hidden="true"> →</span>
              </ButtonBase>
            )}
            {(notice.details || actions.length > 1) && (
              <ButtonBase
                aria-expanded={expanded}
                aria-controls={detailsId}
                onClick={() => {
                  read();
                  setExpanded(!expanded);
                }}
              >
                {text("details")}
                <span aria-hidden="true"> {expanded ? "−" : "+"}</span>
              </ButtonBase>
            )}
          </div>
        )}
        {expanded && (
          <div className="cb-history-details" id={detailsId}>
            {notice.details && (
              <>
                <pre className="cb-selectable">{notice.details}</pre>
                <ButtonBase
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(notice.details);
                      setCopied(text("copied"));
                    } catch {
                      setCopied(text("copyFailed"));
                    }
                  }}
                >
                  {text("copy")}
                </ButtonBase>
                <span role="status">{copied}</span>
              </>
            )}
            {actions.slice(1).map((action) => (
              <ButtonBase key={action} onClick={() => act(action)}>
                {text(action)}
              </ButtonBase>
            ))}
          </div>
        )}
      </div>
    </article>
  );
}
export function NotificationHistory() {
  const {
    history,
    historyOpen: open,
    setHistoryOpen: setOpen,
  } = useContext(Context);
  const [filter, setFilter] = useState<"all" | "pending">("all");
  const [cleared, setCleared] = useState(0);
  const [clearing, setClearing] = useState(false);
  const [position, setPosition] = useState({
    top: 64,
    right: 16,
    maxHeight: 590,
  });
  const undoTimer = useRef<number>();
  const ref = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  useT();
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const bounds = trigger.current?.getBoundingClientRect();
      if (!bounds) return;
      const top = bounds.bottom + 8;
      setPosition({
        top,
        right: Math.max(16, window.innerWidth - bounds.right),
        maxHeight: Math.max(120, Math.min(590, window.innerHeight - top - 12)),
      });
    };
    place();
    const observer = new ResizeObserver(place);
    if (trigger.current) observer.observe(trigger.current);
    window.addEventListener("resize", place);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
    };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const click = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) setOpen(false);
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    document.addEventListener("mousedown", click);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("mousedown", click);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  useEffect(() => () => window.clearTimeout(undoTimer.current), []);
  const pending = history.filter(isPending).length;
  const unread = history.filter(
    (n) => !n.readAt && n.kind !== "progress",
  ).length;
  const completed = history.filter(
    (n) => !isPending(n) && n.kind !== "progress",
  ).length;
  const rows = [...history]
    .filter((n) => filter === "all" || isPending(n))
    .sort(
      (a, b) =>
        Number(isPending(b)) - Number(isPending(a)) ||
        (b.updatedAt || b.createdAt) - (a.updatedAt || a.createdAt),
    );
  return (
    <div ref={ref} className="cb-notification-history">
      <Button context="toolbar" iconOnly
        ref={trigger}
        className="cb-notification-trigger"
        aria-label={`${text("history")}${unread ? ` · ${unread} ${text("unread")}` : ""}`}
        title={text("history")}
        aria-expanded={open}
        aria-controls={open ? "notification-history-panel" : undefined}
        onClick={() => setOpen(!open)}
      >
        <Icon name="bell" size={18} />
        {unread > 0 && (
          <span className="cb-notification-unread-dot" aria-hidden="true" />
        )}
      </Button>
      {open && (
        <section
          id="notification-history-panel"
          className="cb-notification-history-panel"
          style={position}
          aria-label={text("history")}
        >
          <header className="cb-history-header">
            <div>
              <h2>{text("history")}</h2>
              <p>
                {pending ? `${pending} ${text("pending")}` : text("noPending")}
                {unread > 0 && ` · ${unread} ${text("unread")}`}
              </p>
            </div>
            <ButtonBase
              className="cb-history-close"
              aria-label={text("close")}
              onClick={close}
            >
              ×
            </ButtonBase>
          </header>
          <div className="cb-history-toolbar">
            <div className="cb-history-filters" aria-label={text("history")}>
              <ButtonBase
                aria-pressed={filter === "all"}
                onClick={() => setFilter("all")}
              >
                {text("all")}
              </ButtonBase>
              <ButtonBase
                aria-pressed={filter === "pending"}
                onClick={() => setFilter("pending")}
              >
                {text("pending")}
                {pending > 0 && <span>{pending}</span>}
              </ButtonBase>
            </div>
            <ButtonBase
              disabled={!unread}
              onClick={() =>
                void invoke("notification_mark_read", { id: null }).catch(
                  console.error,
                )
              }
            >
              {text("markRead")}
            </ButtonBase>
          </div>
          <div className="cb-history-list">
            {!rows.length ? (
              <div className="cb-history-empty">
                <Icon name="bell" size={26} />
                <h3>{text(filter === "pending" ? "noPending" : "empty")}</h3>
                <p>{text("emptyHint")}</p>
              </div>
            ) : (
              rows.map((notice) => (
                <HistoryRow
                  key={notice.id}
                  notice={notice}
                  onAction={() => setOpen(false)}
                />
              ))
            )}
          </div>
          <footer className="cb-history-footer">
            {cleared ? (
              <>
                <span role="status">{text("cleared", { n: cleared })}</span>
                <ButtonBase
                  disabled={clearing}
                  onClick={async () => {
                    setClearing(true);
                    try {
                      await invoke("notification_restore_completed");
                      setCleared(0);
                      window.clearTimeout(undoTimer.current);
                    } finally {
                      setClearing(false);
                    }
                  }}
                >
                  {text("undo")}
                </ButtonBase>
              </>
            ) : (
              <>
                <span>{text("session")}</span>
                <ButtonBase
                  disabled={clearing || !completed || filter === "pending"}
                  onClick={async () => {
                    setClearing(true);
                    try {
                      const count = await invoke<number>(
                        "notification_clear_completed",
                      );
                      setCleared(count);
                      window.clearTimeout(undoTimer.current);
                      undoTimer.current = window.setTimeout(
                        () => setCleared(0),
                        10000,
                      );
                    } finally {
                      setClearing(false);
                    }
                  }}
                >
                  {text("clear")}
                </ButtonBase>
              </>
            )}
          </footer>
        </section>
      )}
    </div>
  );
}

export function NotificationWindow() {
  useEffect(() => {
    initLang();
    const apply = (theme: string) =>
      document.documentElement.classList.toggle(
        "dark",
        theme === "dark" ||
          (theme === "system" &&
            matchMedia("(prefers-color-scheme: dark)").matches),
      );
    let alive = true;
    void ipc.getUiSettings().then((settings) => {
      if (alive) apply(settings.theme);
    });
    const off = ipc.onTheme(apply);
    const media = matchMedia("(prefers-color-scheme: dark)");
    const changed = () =>
      void ipc.getUiSettings().then((settings) => apply(settings.theme));
    media.addEventListener("change", changed);
    return () => {
      alive = false;
      void off.then((f) => f());
      media.removeEventListener("change", changed);
    };
  }, []);
  return <NotificationProvider />;
}

export function useNotificationNavigation(navigate: (page: string) => void) {
  const { navigateTo } = useContext(Context);
  useEffect(() => {
    if (!navigateTo) return;
    navigate(navigateTo);
    void invoke("notification_ack_navigation").catch(console.error);
  }, [navigateTo]);
}
