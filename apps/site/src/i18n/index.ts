// 构建期 i18n：key 语义化唯一，文案按语言分文件（zh.ts / en.ts）。
// 组件内消费：i18n.t(lang, "guide.step1.title")。
import zh from "./zh";
import en from "./en";

export type Lang = "zh" | "en";
export type Locale = "zh-CN" | "en";

export const LOCALES: Record<Lang, Locale> = { zh: "zh-CN", en: "en" };

const DICTS: Record<Lang, Record<string, string>> = { zh, en };

/** 取文案：en 缺失键回退中文（zh 为 source of truth）。 */
export function t(lang: Lang, key: string): string {
  if (lang === "en") return en[key] ?? zh[key] ?? key;
  return zh[key] ?? key;
}

/**
 * 组件内消费入口：`const i18n = createI18n(lang);` 之后直接 `i18n("home.1")`。
 * 语种在创建时绑定，调用处只需 key——一眼看出取的是哪条文案。
 */
export function createI18n(lang: Lang): (key: string) => string {
  return (key: string) => t(lang, key);
}

/** 同一页面的另一语言路径；英文在根路径，中文在 /zh/*。 */
export function pathFor(lang: Lang, path: string): string {
  // trailingSlash: never —— 一律不带尾斜杠（/zh、/zh/features）
  if (lang === "zh") return path === "/" ? "/zh" : `/zh${path}`;
  return path;
}

export function other(lang: Lang): Lang {
  return lang === "zh" ? "en" : "zh";
}
