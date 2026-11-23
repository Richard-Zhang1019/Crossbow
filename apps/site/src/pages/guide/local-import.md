---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: local-import
---

## Import a source

Open **Configuration → Profiles → Import**. Paste a subscription URL, Base64 subscription content, or one or more node share links. You can also import a configuration file or drop it into the window.

1. Wait for **Import compatibility** after pasting.
2. Check the selected core, node count and unsupported parameters. A remote URL is not fetched during this initial check; importing still needs to fetch and validate it.
3. Confirm the import. Share-link lists are handled as a batch; unsupported entries are not silently discarded.
4. Select the intended profile in the top-right switcher, then open [Nodes](/guide/nodes). Successful import does not mean the outbound has switched.

## Formats and compatibility

Crossbow parses SS, VMess, Trojan, VLESS, Hy2/Hysteria2 and TUIC links and Base64 content locally, without a third-party conversion service. **Supporting a format does not mean every parameter is supported**. Follow the inspection result for your current core and version.

mihomo and sing-box have different native formats and conversion subsets. Native sing-box JSON cannot be used as a mihomo profile. Unconvertible DNS, hosts and other semantics are rejected explicitly rather than silently changing behavior.

Key boundaries in the v0.15.3 inspection matrix:

| Share-link capability | mihomo | sing-box |
| --- | --- | --- |
| Hy2 | Verified subset | Limited support |
| TUIC | Verified subset | Current conversion unsupported |
| VLESS REALITY / vision | Verified subset | Current conversion unsupported |

This describes Crossbow’s import/conversion path, not every capability of an upstream core. Other fields still follow the current inspection result.

## When import fails

Correct the protocol, escaping or parameters across the whole list and retry. For a fetch failure, check the subscription source and local network. Keep credential-bearing URLs private.

[Browser deep links](/guide/deeplink) and [portable packages](/guide/migration) use different entry points.
