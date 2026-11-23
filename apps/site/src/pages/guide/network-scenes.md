---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: network-scenes
---

## Assign an action to Wi-Fi

Open **Settings → Network & core → Network scenes**. Refresh the current network, add its Wi-Fi name, choose **Direct** or **Proxy** and save. Matching uses the actual SSID, not a friendly annotation.

Joining a configured network applies its takeover action. Unconfigured networks follow global settings. Leaving a scene restores the takeover state from before entry.

## Preferences and permissions

**Proxy** follows the previous takeover preference and does not additionally enable TUN. **Direct** is a takeover action, not a rewrite of every subscription rule.

On macOS, reading Wi-Fi names may require Location permission. Use the page's permission request, check System Settings and refresh. Disconnected Wi-Fi, unavailable names and unsupported platforms have distinct states. An unreadable name is not a confirmed rule match.

## When states disagree

Distinguish saved rules, the current connection and a pending scene application. If saving or switching fails, inspect notifications and the actual system proxy/TUN switches before refreshing. Do not stack repeated toggles over an unfinished operation.

See [system proxy and TUN](/guide/proxy-vs-tun) for their different scopes.
