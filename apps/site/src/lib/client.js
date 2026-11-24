// Shared browser behavior for the site. Astro's ClientRouter keeps a single
// document across navigations, so every initializer below is idempotent and
// runs again on astro:page-load after the new page is swapped in.

const THEME_KEY = "crossbow.site.theme";
const LANGUAGE_KEY = "crossbow.site.language";
const SUN_ICON = '<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4.2" /><path d="M12 2.5v2.6M12 18.9v2.6M2.5 12h2.6M18.9 12h2.6M5 5l1.9 1.9M17.1 17.1 19 19M19 5l-1.9 1.9M6.9 17.1 5 19" /></svg>';
const MOON_ICON = '<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" /></svg>';

const readTheme = () => {
  try {
    return localStorage.getItem(THEME_KEY) || "system";
  } catch {
    return "system";
  }
};

function closePreferences() {
  document.querySelectorAll("[data-pref]").forEach((anchor) => {
    const pop = anchor.querySelector(".preference-popover");
    if (pop && !pop.hidden) {
      pop.hidden = true;
      anchor.querySelector("[data-pref-button]")?.setAttribute("aria-expanded", "false");
    }
  });
}

function setMenu(open) {
  const siteNav = document.getElementById("site-nav");
  const menuButton = document.querySelector(".menu-button");
  if (!siteNav || !menuButton) return;
  siteNav.classList.toggle("nav-open", open);
  menuButton.setAttribute("aria-expanded", String(open));
  const main = document.getElementById("main");
  const footer = document.querySelector(".site-footer");
  if (main) main.inert = open;
  if (footer) footer.inert = open;
  document.body.style.overflow = open ? "hidden" : "";
}

function initHeader() {
  const root = document.documentElement;
  const paint = () => {
    const stored = readTheme();
    const dark = stored === "dark" || (stored !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
    root.dataset.theme = dark ? "dark" : "light";
    document.querySelector('meta[name="theme-color"]')?.setAttribute("content", dark ? "#1a1c21" : "#ffffff");
    const icon = document.querySelector("[data-theme-icon]");
    if (icon) icon.innerHTML = dark ? MOON_ICON : SUN_ICON;
    const radio = document.querySelector(`[data-theme-pop] input[value="${stored}"]`);
    if (radio) radio.checked = true;
  };
  paint();

  document.querySelectorAll("[data-pref]").forEach((anchor) => {
    const button = anchor.querySelector("[data-pref-button]");
    const pop = anchor.querySelector(".preference-popover");
    if (!button || !pop) return;
    const setOpen = (open) => {
      pop.hidden = !open;
      button.setAttribute("aria-expanded", String(open));
    };
    button.onclick = () => {
      const next = pop.hidden;
      closePreferences();
      setOpen(next);
    };
  });

  const themePop = document.querySelector("[data-theme-pop]");
  if (themePop) {
    themePop.onchange = (e) => {
      const value = e.target.value;
      try {
        localStorage.setItem(THEME_KEY, value);
      } catch {}
      paint();
    };
  }
  const langPop = document.querySelector("[data-lang-pop]");
  if (langPop) {
    langPop.onchange = (e) => {
      const value = e.target.value;
      try {
        localStorage.setItem(LANGUAGE_KEY, value === "en" ? "en" : "zh-CN");
      } catch {}
      const form = document.querySelector("[data-lang-pop]");
      const target = value === "zh" ? form?.dataset.zhHref : form?.dataset.enHref;
      if (target) {
        location.href = new URL(target + location.search + (form?.dataset.resetHash ? "" : location.hash), location.origin).href;
      }
    };
  }

  const menuButton = document.querySelector(".menu-button");
  if (menuButton) {
    menuButton.onclick = () => setMenu(!document.getElementById("site-nav")?.classList.contains("nav-open"));
  }
  document.querySelectorAll("#site-nav a").forEach((a) => {
    a.onclick = () => setMenu(false);
  });

  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.onclick = () => {
      const share = new URL(button.dataset.copy ?? "", location.origin).href;
      navigator.clipboard?.writeText(share).then(() => {
        const previous = button.textContent;
        button.textContent = "✓";
        setTimeout(() => {
          button.textContent = previous;
        }, 1500);
      }, () => {});
    };
  });
  document.querySelectorAll("[data-copy-code]").forEach((button) => {
    button.onclick = () => {
      const pre = button.closest(".code-wrap")?.querySelector("pre");
      if (!pre) return;
      navigator.clipboard?.writeText(pre.textContent ?? "").then(() => {
        const previous = button.textContent;
        button.textContent = "✓";
        setTimeout(() => {
          button.textContent = previous;
        }, 1500);
      }, () => {});
    };
  });

  matchMedia("(min-width: 1001px)").onchange = (e) => {
    if (e.matches) setMenu(false);
  };
}

/* ---- documentation workspace ---- */
let docsMedia = null;
function initDocs() {
  const sidebar = document.querySelector(".docs-sidebar");
  const search = document.querySelector("[data-doc-search]");
  if (!sidebar && !search) return;

  if (sidebar) {
    sidebar.onkeydown = (e) => {
      const mobile = matchMedia("(max-width: 800px)");
      if (e.key === "Escape" && mobile.matches && sidebar.open) {
        sidebar.open = false;
        sidebar.querySelector("summary")?.focus();
      }
    };
    if (!docsMedia) {
      docsMedia = matchMedia("(max-width: 800px)");
    }
  }
  if (docsMedia) {
    docsMedia.onchange = () => {
      if (sidebar) sidebar.open = !docsMedia.matches;
    };
    if (sidebar && docsMedia.matches) sidebar.open = false;
  }

  if (search) {
    search.oninput = () => {
      const q = search.value.trim().toLocaleLowerCase();
      let total = 0;
      document.querySelectorAll("[data-doc-group]").forEach((group) => {
        let shown = 0;
        group.querySelectorAll("[data-doc-topic]").forEach((item) => {
          const match = (item.dataset.search ?? "").toLocaleLowerCase().includes(q);
          item.hidden = !match;
          if (match) shown++;
        });
        group.hidden = shown === 0;
        total += shown;
      });
      const empty = document.querySelector("[data-doc-empty]");
      if (empty) empty.hidden = total !== 0;
    };
  }
}

/* Keep the directory position between route changes (real static navigations). */
function initDocsScroll() {
  const pane = document.querySelector(".docs-sidebar-body");
  if (!pane) return;
  try {
    pane.scrollTop = Number(sessionStorage.getItem("crossbow.docs.scroll") || 0);
  } catch {}
  const selected = pane.querySelector('[aria-current="page"]');
  if (selected) {
    const a = selected.getBoundingClientRect();
    const b = pane.getBoundingClientRect();
    if (a.top < b.top || a.bottom > b.bottom) pane.scrollTop += a.top - b.top - 80;
  }
  document.querySelectorAll(".docs-navigation a").forEach((a) => {
    a.onclick = () => {
      try {
        sessionStorage.setItem("crossbow.docs.scroll", String(pane.scrollTop));
      } catch {}
    };
  });
}

/* Legacy hash links: /guide#quick-start and /features#support-matrix land on routes. */
function migrateLegacyHash() {
  const el = document.querySelector("[data-doc-legacy]");
  if (!el) return;
  const id = location.hash.slice(1);
  if (!id) return;
  const sections = JSON.parse(el.dataset.sections || "[]");
  if (sections.includes(id)) location.replace(`${el.dataset.root}/${id}${location.search}`);
}

/* Hero motion pause toggle. */
let heroPaused = document.hidden;
let heroRender = null;
function initHero() {
  const lane = document.querySelector("[data-packet]");
  const button = document.querySelector("[data-pause]");
  if (!lane || !button) return;
  heroPaused = document.hidden;
  heroRender = () => {
    lane.style.animationPlayState = heroPaused ? "paused" : "running";
    button.setAttribute("aria-pressed", String(heroPaused));
    button.firstElementChild.textContent = heroPaused ? "▷" : "Ⅱ";
    button.lastChild.textContent = heroPaused
      ? button.dataset.resumeText ?? " Resume motion"
      : button.dataset.pauseText ?? " Pause motion";
  };
  button.onclick = () => {
    heroPaused = !heroPaused;
    heroRender();
  };
  heroRender();
}

function init() {
  initHeader();
  initDocs();
  initDocsScroll();
  migrateLegacyHash();
  initHero();
}

/* Document-level listeners are installed once; per-page nodes use on* handlers. */
document.addEventListener("click", (e) => {
  if (!e.target.closest("[data-pref]")) closePreferences();
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closePreferences();
    setMenu(false);
  }
});
document.addEventListener("visibilitychange", () => {
  if (heroRender) {
    heroPaused = document.hidden;
    heroRender();
  }
});
window.addEventListener("hashchange", migrateLegacyHash);

document.addEventListener("astro:page-load", init);
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", init, { once: true });
}
