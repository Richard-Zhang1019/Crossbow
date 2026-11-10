/** 统一线性图标（1.6 描边，对齐设计稿 v2）。 */
export function Icon({
  name,
  size = 15,
  stroke = "currentColor",
  strokeWidth = 1.6,
}: {
  name: string;
  size?: number;
  stroke?: string;
  strokeWidth?: number;
}) {
  const paths: Record<string, React.ReactNode> = {
    automatic: <><path d="M20 8a8 8 0 0 0-13-3L3 9m0-6v6h6M4 16a8 8 0 0 0 13 3l4-4m0 6v-6h-6"/></>,
    bell: (
      <>
        <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9" />
        <path d="M10 21h4" />
      </>
    ),
    device: (
      <>
        <rect x="3" y="4" width="18" height="13" rx="2" />
        <path d="M8 21h8m-4-4v4" />
      </>
    ),
    globe: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18M12 3c5 5 5 13 0 18M12 3c-5 5-5 13 0 18M5 6.5c4 3 10 3 14 0M5 17.5c4-3 10-3 14 0" />
      </>
    ),
    sliders: (
      <>
        <path d="M3 6h18M3 12h18M3 18h18" />
        <path d="M7 3v6m10 0v6M9 15v6" />
      </>
    ),
    document: (
      <>
        <path d="M6 3h8l4 4v14H6zM14 3v5h4M9 12h6M9 16h6" />
      </>
    ),
    chevron: <path d="m9 5 7 7-7 7" />,
    "arrow-right": <path d="M5 12 H19 M14 7 L19 12 L14 17" />,
    close: <path d="M6 6L18 18M18 6L6 18"/>,
    minus: <path d="M5 12H19"/>,
    check: <path d="M5 12 L10 17 L19 7" />,
    download: <path d="M12 3 V15 M7 10 L12 15 L17 10 M4 17 V21 H20 V17" />,
    activity: <path d="M3 12 L10 12 L13 5 L17 19 L20 12 L21 12" />,
    bolt: <path d="M13 2 L5 13 L11 13 L9 22 L19 9 L13 9 Z" />,
    cube: (
      <>
        <path d="M12 3 L20 7 L20 17 L12 21 L4 17 L4 7 Z" />
        <path d="M4 7 L12 11 L20 7" />
        <path d="M12 11 L12 21" />
      </>
    ),
    lines: <path d="M4 6 L20 6 M4 12 L20 12 M4 18 L20 18" />,
    grid: (
      <path d="M4 5 L16 5 L16 11 L4 11 Z M16 9 L20 9 L20 15 L16 15 M4 15 L12 15 L12 19 L4 19 Z" />
    ),
    gear: (
      <>
        <circle cx="12" cy="12" r="3.2" />
        <path d="M12 2.5 L12 5.5 M12 18.5 L12 21.5 M2.5 12 L5.5 12 M18.5 12 L21.5 12 M5 5 L7 7 M17 17 L19 19 M19 5 L17 7 M7 17 L5 19" />
      </>
    ),
    crossbow: <path d="M5 19 L19 5 M5 5 L12 12" />,
    pulse: <path d="M3 12 L7 12 L9 6 L12 18 L15 9 L17 12 L21 12" />,
    search: (
      <>
        <circle cx="11" cy="11" r="6" />
        <path d="M20 20 L15.5 15.5" />
      </>
    ),
    pause: <path d="M9 5 L9 19 M15 5 L15 19" />,
    play: <path d="M7 5 L19 12 L7 19 Z" />,
    refresh: (
      <>
        <path d="M20 11 A8 8 0 1 0 20 13" />
        <path d="M20 4 L20 11 L13 11" />
      </>
    ),
    trash: (
      <>
        <path d="M4 7 L20 7 M9 7 L9 4 L15 4 L15 7 M6 7 L7 20 L17 20 L18 7" />
      </>
    ),
    upload: (
      <>
        <path d="M12 16 L12 4 M7 9 L12 4 L17 9" />
        <path d="M4 16 L4 20 L20 20 L20 16" />
      </>
    ),
  };
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke={stroke}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ flex: "none" }}
    >
      {paths[name] ?? null}
    </svg>
  );
}
