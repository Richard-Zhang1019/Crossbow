// 首屏前的外观引导：主题必须先于首次绘制落到 <html> 上（语言由 URL 决定，
// 各语言页面在构建期写死 html lang）。此脚本以 is:inline 形式内联进 Layout，
// 并被 tests/bootstrap.test.mjs 直接执行，保持存储被拒等场景的行为契约。
export const BOOTSTRAP = String.raw`(() => {
  let theme = "system";
  try {
    theme = localStorage.getItem("crossbow.site.theme") || "system";
  } catch {}
  const dark = theme === "dark" || (theme !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.querySelector('meta[name="theme-color"]').content = dark ? "#1a1c21" : "#ffffff";
})();`;
