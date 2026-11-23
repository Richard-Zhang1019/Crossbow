---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: node-tests
---

## Run component tests explicitly

Open **Nodes → Quality history → Component tests**. Choose a concrete node and **Connectivity** or **Bounded throughput**, then start it explicitly. Opening the panel or reading history does not run tests.

Connectivity separates the **TCP destination tunnel, HTTPS request and UDP DNS**. Fixed targets are Cloudflare HTTPS and `1.1.1.1:53`, with at most one HTTPS request, three UDP datagrams and a 30-second overall limit.

## Traffic and interpretation

A bounded transfer downloads at most 4 MiB and uploads 1 MiB of test body, plus encryption, connection, DNS and response overhead. It may consume subscription traffic. This measures end-to-end effective throughput including setup and target response, not the line's bandwidth ceiling.

Incomplete downloads produce no rate. Failed uploads may have sent partial payload; do not assume zero bytes. A single unverified HTTPS/UDP result describes this target and attempt, not permanent node failure.

## Support and cancellation

Only inline concrete nodes and their dialer chain are supported. Groups and provider-only nodes report unsupported. Tests use an isolated core and system DNS, but existing host TUN/VPN software may still affect the route.

Cancel or close the panel to cancel unfinished tests. Component results retain up to 128 entries for seven days and share the quality recording/clear controls. They are separate from [routine latency history](/guide/node-quality).
