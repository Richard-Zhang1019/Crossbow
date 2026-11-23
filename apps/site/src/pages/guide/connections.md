---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: connections
---

## Inspect where a connection went

Open **Monitor → Connections** and view the list or process groups. Search, sort or pause refresh. Pausing freezes the display, not the network connections.

Open a row to inspect its destination, network, type, matched rule, outbound chain, process and ID. Copy a field or all details. If the connection has left the list, the last snapshot is historical, not proof it remains active.

## Turn observations into rules

A row's action menu can close a connection or create a rule for an identified process. The detail footer can prefill a domain/IP rule. Review its target and policy before saving; opening a dialog alone does not change routing.

**Unknown process** means this metadata has no usable identity. Permissions, takeover mode and platform affect identification. Static rule explanations do not replace the matched rule and chain returned by the core.

## Logs versus statistics

[Local statistics](/guide/statistics) aggregate estimated historical traffic from snapshots. [Diagnostics and logs](/guide/troubleshooting) investigate events. Their scopes differ; a live connection total is not subscription billing data.
