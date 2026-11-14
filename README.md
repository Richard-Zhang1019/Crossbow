# Crossbow

<p align="center">
  <a href="https://crossbow-client.netlify.app">
    <img width="60" src="./design/logo/current/app-icon.svg" />
  </a>
</p>
<p align="center">
    English | <a href="/README.zh-CN.md">简体中文</a>
</p>

Crossbow is an open-source proxy client for macOS and Windows, built with Tauri 2, React and
Rust, supporting the mihomo and sing-box dual cores. It puts subscriptions, node selection,
system proxy, TUN, connections and logs in a single interface, with live up/down speeds in the
tray / menu bar.

<img width="1120" height="720" alt="image" src="https://github.com/user-attachments/assets/29a26e8a-b8ce-425b-9f17-9a228d83d211" />

**Current version:** [v0.14.4](https://github.com/Richard-Zhang1019/Crossbow/releases/tag/v0.14.4).
Available as a macOS Apple Silicon DMG, a Windows x64 installer, and an experimental Linux
x86_64 archive. Windows is under active development; TUN uses the controlled system service
bundled with the installer. For compatibility issues, please open an issue with the app
version, notification details and diagnostics.

Linux is being adapted in stages for **CachyOS x86_64 + niri/Wayland**. Plain HTTP/SOCKS proxy
works independently; stage two adds an optional mihomo TUN service that requires separate
installation and authorization, and refuses takeover when other proxies, VPNs or policy routing
are present. Linux packages ship the desktop app, mihomo and an optional TUN service install
script; system proxy, AppImage/DEB and auto-update are not available yet. Build notes and
field records: [Linux testing](docs/LINUX-TESTING.md); service installation, permissions and
recovery boundaries: [Linux network takeover](docs/LINUX-NETWORK.md).

## Install & use

| Platform | Package | Notes |
|---|---|---|
| macOS Apple Silicon | [Download DMG](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.14.4/Crossbow_0.14.4_aarch64.dmg) | Drag into Applications; Developer ID signed and notarized |
| Windows x64 | [Download installer](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.14.4/Crossbow_0.14.4_x64-setup.exe) | No Windows code signing yet — verify the official source and checksums |
| Linux x86_64 (experimental) | [Download tar.gz](https://github.com/Richard-Zhang1019/Crossbow/releases/download/v0.14.4/Crossbow_0.14.4_linux_x86_64.tar.gz) | Extract and run `run.sh` as a regular user; dependencies and TUN setup in the bundled `README.txt` |

Downloads and release notes live on [GitHub Releases](https://github.com/Richard-Zhang1019/Crossbow/releases/latest);
third-party components and licenses are listed in [THIRD-PARTY-NOTICES](docs/THIRD-PARTY-NOTICES.md).
Check for updates in-app under "Settings → Software update" and follow the in-app prompts.
Release files ship with `SHA256SUMS`; `main` may contain unreleased changes.

1. Import a subscription link or config file and pick a node on the "Proxies" page; you can
   also switch nodes and check latency from the tray menu.
2. For daily use, enable the system proxy on the home page. To capture apps that ignore the
   system proxy, enable TUN in "Settings" on macOS.
3. TUN and the system proxy can be enabled together and controlled independently. First-time
   TUN enablement needs admin authorization. Turn off other proxies' or VPNs' TUN first to
   avoid routing conflicts.

v0.14.3 adds the MIT LICENSE; Windows silent installs no longer abort when service
registration is restricted by the environment (a prerequisite fix for the winget listing).
Windows GUI and real-network behavior still need field verification — see
[Windows testing](docs/WINDOWS-TESTING.md). If the explicit proxy works but your browser
doesn't, check whether the browser uses the system proxy.

When testing network configs on macOS, read [TUN offline recovery](docs/TUN-RECOVERY.md)
first; recovery commands need a still-running backup proxy. macOS configs and logs live in
`~/Library/Application Support/com.richard.crossbow/`. Data from the legacy
`com.crossbow.app` directory migrates automatically; backing up the data directory before
upgrading is still recommended.

## Development

Requires Node.js 20+, pnpm and Rust stable; local macOS builds also need Xcode Command Line
Tools.

```bash
pnpm install
pnpm tauri dev
pnpm --filter @crossbow/site build
```

To package the desktop app locally (without updater artifacts that require release signing):

```bash
cd apps/desktop
npm run tauri build -- --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

Repository layout:

```text
crates/crossbow-core  shared config, override and storage logic
apps/desktop          macOS / Windows desktop app
apps/site             website (Netlify)
apps/ios              iOS project skeleton
docs                  design docs, roadmap and recovery guides
```

During desktop development, point at a local core with
`CROSSBOW_MIHOMO_BIN=/path/to/mihomo pnpm tauri dev`. Design docs, scope and plans:
[DESIGN](docs/DESIGN.md), [MVP](docs/MVP.md) and [ROADMAP](docs/ROADMAP.md); macOS release
steps: [RELEASE](docs/RELEASE.md).
Desktop core integration: [mihomo control protocol](docs/MIHOMO-CONTROL.md).

## Anonymous usage statistics

On desktop versions that support usage statistics, this feature is enabled by default. Crossbow sends only the date, platform, app version, and a random identifier that changes daily. The identifier is used only to deduplicate daily reports and cannot track an installation across days.

Reports do not include subscription URLs, proxy nodes, browsing history, account details, or hardware identifiers. The metrics database does not store IP addresses. You can turn reporting off at any time in **Settings → Application & updates → Anonymous usage statistics**.

## Thanks to all contributors

<a href="https://github.com/Richard-Zhang1019/Crossbow/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=Richard-Zhang1019/Crossbow" />
</a>

## License

MIT — see [LICENSE](LICENSE). mihomo and sing-box run as separate sidecar processes (not
linked) and remain under their own licenses.
