import type { createI18n } from "../i18n";

type I18n = ReturnType<typeof createI18n>;

/** Feature sections for the documentation-style feature page (#653). */
export function featureData(i18n: I18n) {
  const FEATURES = [
    {
      id: "support-matrix", tag: i18n("features.1"), title: i18n("features.2"), matrix: true,
      summary: i18n("features.3"),
      detail: [i18n("features.4")],
    },
    {
      id: "nodes", tag: i18n("features.5"), title: i18n("features.6"),
      summary: i18n("features.7"),
      detail: [
        i18n("features.8"),
        i18n("features.9"),
        i18n("features.10"),
        i18n("features.11"),
        i18n("features.110"),
      ],
    },
    {
      id: "subscription", tag: i18n("features.12"), title: i18n("features.13"),
      summary: i18n("features.14"),
      detail: [
        i18n("features.15"),
        i18n("features.16"),
        i18n("features.17"),
        i18n("features.18"),
        i18n("features.19"),
        i18n("features.20"),
      ],
    },
    {
      id: "sysproxy-guard", tag: i18n("features.21"), title: i18n("features.22"),
      summary: i18n("features.23"),
      detail: [
        i18n("features.24"),
        i18n("features.25"),
        i18n("features.26"),
        i18n("features.27"),
        i18n("features.107"),
        i18n("features.108"),
      ],
    },
    {
      id: "tun", tag: i18n("features.28"), title: i18n("features.29"),
      summary: i18n("features.30"),
      detail: [
        i18n("features.31"),
        i18n("features.32"),
        i18n("features.33"),
        i18n("features.34"),
      ],
    },
    {
      id: "diagnostics", tag: i18n("features.35"), title: i18n("features.36"),
      summary: i18n("features.37"),
      detail: [
        i18n("features.38"),
        i18n("features.39"),
        i18n("features.40"),
        i18n("features.104"),
        i18n("features.105"),
        i18n("features.106"),
      ],
    },
    {
      id: "js-override", tag: i18n("features.41"), title: i18n("features.42"),
      summary: i18n("features.43"),
      detail: [
        i18n("features.44"),
        i18n("features.45"),
        i18n("features.46"),
        i18n("features.47"),
      ],
    },
    {
      id: "lightweight", tag: i18n("features.48"), title: i18n("features.49"),
      summary: i18n("features.50"),
      detail: [
        i18n("features.51"),
        i18n("features.52"),
        i18n("features.53"),
      ],
    },
    {
      id: "observability", tag: i18n("features.54"), title: i18n("features.55"),
      summary: i18n("features.56"),
      detail: [
        i18n("features.57"),
        i18n("features.58"),
        i18n("features.59"),
        i18n("features.60"),
        i18n("features.61"),
        i18n("features.62"),
      ],
    },
    {
      id: "engines", tag: i18n("features.63"), title: i18n("features.64"),
      summary: i18n("features.65"),
      detail: [
        i18n("features.66"),
        i18n("features.67"),
        i18n("features.68"),
        i18n("features.69"),
      ],
    },
    {
      id: "notifications", tag: i18n("features.70"), title: i18n("features.71"),
      summary: i18n("features.72"),
      detail: [
        i18n("features.73"),
        i18n("features.74"),
        i18n("features.75"),
        i18n("features.76"),
      ],
    },
    {
      id: "snapshots", tag: i18n("features.77"), title: i18n("features.78"),
      summary: i18n("features.79"),
      detail: [
        i18n("features.80"),
        i18n("features.81"),
        i18n("features.82"),
      ],
    },
    {
      id: "config-package", tag: i18n("features.135"), title: i18n("features.135"),
      summary: i18n("features.136"),
      detail: [
        i18n("features.137"),
        i18n("features.103b"),
      ],
    },
    {
      id: "updates", tag: i18n("features.83"), title: i18n("features.84"),
      summary: i18n("features.85"),
      detail: [
        i18n("features.86"),
        i18n("features.87"),
        i18n("features.88"),
      ],
    },
    {
      id: "preferences", tag: i18n("features.89"), title: i18n("features.90"),
      summary: i18n("features.91"),
      detail: [
        i18n("features.92"),
        i18n("features.93"),
        i18n("features.94"),
      ],
    },
    {
      id: "linux", tag: "Linux", title: i18n("features.95"),
      summary: i18n("features.96"),
      detail: [
        i18n("features.97"),
        i18n("features.98"),
        i18n("features.99"),
      ],
    },
  ];

  // 四组归类（#365）：锚点 id 保持稳定，外链不漂移
  const GROUPS: { label: string; ids: string[] }[] = [
    { label: i18n("features.100"), ids: ["nodes", "subscription", "preferences"] },
    { label: i18n("features.101"), ids: ["sysproxy-guard", "tun", "engines", "js-override"] },
    { label: i18n("features.102"), ids: ["diagnostics", "observability", "notifications"] },
    { label: i18n("features.103"), ids: ["lightweight", "snapshots", "updates", "linux"] },
  ];
  const byId = Object.fromEntries(FEATURES.map((f) => [f.id, f]));
  const groupLabel = (id: string) => GROUPS.find((g) => g.ids.includes(id))?.label ?? "";
  return { FEATURES, GROUPS, byId, groupLabel };
}
