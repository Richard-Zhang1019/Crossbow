---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: statistics
---

## Review observed local traffic

Open **Monitor → Stats**, then choose **Today**, **Last 7 days** or **Last 30 days**. Inspect daily totals and the top ten domains and nodes.

Figures are estimated from connection snapshots and stored on this device only. Retention is up to 30 days, shorter at capacity. Long tails become **Others**; tracking is capped at 8192 connections and oversized IDs are skipped. **This is not a subscription billing meter.**

## Pause and clear

**Pause recording** stops new aggregation while retaining existing data. **Resume recording** continues observation. The first snapshot after pause or clear establishes a baseline; unobserved earlier traffic is not backfilled.

**Clear local stats** removes saved statistics and cannot be undone. It is separate from closing connections, clearing logs or clearing node quality history.

## Why totals differ

A subscription may include other devices, protocol overhead or a different billing window. Short-lived connections, sampling and capacity trimming also affect estimates. On a read or save error, inspect the notification state rather than treating old figures as a fresh successful read.
