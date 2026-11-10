/** Theme-aware progress with no platform-native progress styling. */
export function ProgressRing({ value, label, size = 32 }: { value: number | null; label: string; size?: number }) {
  const percent = value !== null && Number.isFinite(value) ? Math.min(100, Math.max(0, value)) : null;
  return <span className={`cb-progress-ring ${percent === null ? "is-indeterminate" : ""}`}
    role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100}
    aria-valuenow={percent ?? undefined} style={{ width: size, height: size }}>
    <svg viewBox="0 0 36 36" fill="none" strokeWidth="2.5" aria-hidden="true">
      <circle className="cb-progress-ring-track" cx="18" cy="18" r="15" />
      <circle className="cb-progress-ring-value" cx="18" cy="18" r="15" pathLength="100"
        strokeDasharray={`${percent ?? 24} 100`} strokeLinecap="round" />
    </svg>
  </span>;
}
