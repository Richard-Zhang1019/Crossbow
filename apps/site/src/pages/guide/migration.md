---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: migration
---

## Export, preview, confirm

Open **Configuration → Profiles → Backup & migration** and choose **Export config package** or **Import config package**. Import first previews added/replaced entries, name conflicts and active-profile selection. Only confirmation writes; cancelling changes nothing.

> A package is **unencrypted JSON** and may contain subscription tokens, node credentials and scripts. Trust the source, store it privately and never attach it to a public issue.

## What v2 carries

| Included | Excluded |
| --- | --- |
| Profiles/cache, overrides with bindings/order, active-profile selection | App-level DNS, core/port choice, system proxy/TUN state and permissions |
| Process rules/order and rule-set metadata with validated cache | Wi-Fi SSID scenes, logs, statistics/diagnostic state and appearance |

**Domain/IP user rules and custom groups are outside the current v2 package scope**. It is not a complete application backup and does not enable takeover on a new machine. DNS and scripts inside raw profile/override content still travel with that content.

## Merge and failure behavior

Matching stable IDs are replaced; unrelated entries remain. Unreplaced local rules precede package rules. Correct name conflicts and invalid cache first. Missing targets never become DIRECT. Confirmation checks the effective overridden configuration and rolls back the batch on failure.

Preview downloads no subscriptions/rules and executes no scripts. Retained update schedules may access the network after confirmation. Packages are capped at 8 MiB; Crossbow writes v2, reads v1/v2 and rejects future format/schema versions. On failure inspect notifications and [snapshots](/guide/recovery) rather than overwriting unresolved state repeatedly.
