---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: dns
---

## Inherit or customize

Open **Settings → Network & core → DNS resolution**. **Inherit profile** keeps the profile behavior. To customize, select that mode, enter upstreams and choose **Save and apply**. A failure keeps the committed settings and editable draft; it does not mean the new settings became active.

| Field | Purpose |
| --- | --- |
| Proxy upstream | Default resolution for proxied traffic; queries follow the MATCH policy by default |
| Direct upstream | Resolution for direct outbounds |
| Bootstrap upstream | Resolves node server names; mihomo falls back to the direct upstream if empty |

## Core-specific limits

**mihomo:** multiple upstreams are supported. Custom values override the same fields from overrides/profile content, while retaining hosts, per-domain policy and fallback. Specific domains may therefore use another upstream.

**sing-box:** one proxy and one direct upstream are supported; the direct upstream must use an IP to avoid a resolution loop. Independent bootstrap configuration is unavailable. Native JSON custom DNS and unconvertible subscription DNS/hosts are explicitly rejected.

IPv6 still follows configuration, runtime detection and TUN DNS/AAAA policy. Saving does not prove all applications use one resolver.

## Verify after saving

Check for apply or rollback errors in notifications. Then compare the same domain and record family in [DNS path diagnostics](/guide/dns-diagnostics).
