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
