import { useEffect } from "react";
import { createRoot } from "react-dom/client";
import App from "../../../../apps/desktop/src/App";
import { NotificationProvider } from "../../../../apps/desktop/src/notifications/NotificationProvider";
import "../../../../apps/desktop/src/index.css";
import "../../../../apps/desktop/src/layouts/Layout.css";

function Preview() {
  useEffect(() => {
    window.dispatchEvent(new CustomEvent("cb:goto", { detail: "proxies" }));
  }, []);

  return <NotificationProvider><App /></NotificationProvider>;
}

createRoot(document.getElementById("root")!).render(<Preview />);
