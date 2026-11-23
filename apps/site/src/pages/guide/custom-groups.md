---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: custom-groups
---

## Create an independent group

Open **Nodes → Custom groups → Add group**. These groups are managed independently; groups supplied by the subscription remain read-only.

1. Check the current source and enter a group name.
2. Choose **Manual selection** or **URL test**.
3. Select concrete nodes or use a **name contains** filter. This filter is not a regular expression.
4. Review the final member preview, resolve missing nodes and save.

> Creating a group does not change the current outbound. Select it explicitly in a [user rule](/guide/user-rules) or [process rule](/guide/process-rules).

## Automatic tests and membership

A URL-test group requires a URL, interval and tolerance. Saving it enables periodic network requests to that URL; it is different from viewing quality history.

Membership currently supports inline concrete nodes in that profile, excluding DIRECT, nested groups and cross-subscription providers. If the source is inactive, removed or has missing members, select the source and review the preview again.
