// Feature catalog: each section owns a real route under /features/<slug> (#653).
import { createI18n } from "../i18n";
import { featureData } from "./features";

export type FeatureLang = "zh" | "en";

const zh = createI18n("zh");
const en = createI18n("en");
const zhData = featureData(zh);
const enData = featureData(en);

const GROUP_OF: Record<string, string> = {
  "support-matrix": "overview",
  nodes: "manage",
  subscription: "manage",
  preferences: "manage",
  "sysproxy-guard": "connection",
  tun: "connection",
  engines: "connection",
  "js-override": "connection",
  diagnostics: "observe",
  observability: "observe",
  notifications: "observe",
  lightweight: "maintenance",
  snapshots: "maintenance",
  "config-package": "maintenance",
  updates: "maintenance",
  linux: "maintenance",
};

// Keep the directory order stable for existing readers and deep links.
const ORDER = [
  "support-matrix",
  "nodes", "subscription", "preferences",
  "sysproxy-guard", "tun", "engines", "js-override",
  "diagnostics", "observability", "notifications",
  "lightweight", "snapshots", "config-package", "updates", "linux",
];

export const featureGroups = [
  { id: "overview", zh: "总览", en: "Overview" },
  { id: "manage", zh: "管理配置", en: "Management" },
  { id: "connection", zh: "连接接管", en: "Connection" },
  { id: "observe", zh: "观测排障", en: "Observability" },
  { id: "maintenance", zh: "维护恢复", en: "Recovery" },
] as const;

export const featureOverview = {
  slug: "",
  group: "overview",
  title: { zh: "功能详解", en: "Features" },
  description: {
    zh: "从订阅与节点，到接管与观测。了解每一步连接由什么组成。",
    en: "From subscriptions and nodes to takeover and observability — what every connection is made of.",
  },
};

export interface FeatureDocument {
  slug: string;
  group: string;
  title: { zh: string; en: string };
  tag: { zh: string; en: string };
  description: { zh: string; en: string };
  detail: { zh: string[]; en: string[] };
  matrix: boolean;
}

export const featureDocuments: FeatureDocument[] = ORDER.map((id) => {
  const z = zhData.byId[id];
  const e = enData.byId[id];
  if (!z || !e) throw new Error(`Unknown feature section: ${id}`);
  return {
    slug: id,
    group: GROUP_OF[id],
    title: { zh: z.title, en: e.title },
    tag: { zh: z.tag, en: e.tag },
    description: { zh: z.summary, en: e.summary },
    detail: { zh: z.detail, en: e.detail },
    matrix: Boolean(z.matrix),
  };
});

export const featureLegacySections = ORDER;

export function featureHref(lang: FeatureLang, slug = "", base = "") {
  return `${base.replace(/\/$/, "")}${lang === "zh" ? "/zh" : ""}/features${slug ? "/" + slug : ""}`;
}

export function featureContent(lang: FeatureLang, slug: string): FeatureDocument {
  const doc = featureDocuments.find((d) => d.slug === slug);
  if (!doc) throw new Error(`Unknown feature section: ${slug}`);
  return doc;
}
