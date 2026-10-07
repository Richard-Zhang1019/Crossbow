import logoGlyph from "./assets/logo-glyph.png";

/** 官网 Logo：v3 渐变十字弓 glyph（透明底 PNG，与 App 图标同源）。 */
export function Logo({ size = 24 }: { size?: number }) {
  return (
    <img
      src={logoGlyph}
      alt=""
      width={size}
      height={size}
      style={{ flex: "none", display: "block" }}
    />
  );
}

export function GitHubIcon() {
  return (
    <svg viewBox="0 0 16 16" width="15" height="15" fill="currentColor">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  );
}

export function DownloadIcon() {
  return (
    <svg
      width="15"
      height="15"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
    >
      <path d="M12 3v12M6 9l6 6 6-6M4 21h16" />
    </svg>
  );
}

export const REPO = "https://github.com/Richard-Zhang1019/Crossbow";
export const RELEASES = `${REPO}/releases`;
export const LATEST = `${REPO}/releases/latest`;
export const VERSION = "v0.3.0";
export const DMG_URL = `${REPO}/releases/download/${VERSION}/Crossbow_0.3.0_aarch64.dmg`;
export const DMG_SHA256 =
  "83cd07d40a115d3f2bae1c328b9a555deadd650503498702a58e178ec45a6102";
export const MANIFEST_URL = `${REPO}/releases/latest/download/latest.json`;


