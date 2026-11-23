---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: node-quality
---

## Review manual measurements

Open **Nodes → Quality history**, then choose a node and target. It records manual node-page measurements without extra background requests and does not drive automatic routing. Unrequested targets remain unsampled.

mihomo primarily tests Google HTTP and reviews a timeout with Cloudflare HTTPS. sing-box tests Google HTTPS only. **Test once** requests only the selected target, without review. Statistics remain separated by target.

## Interpret the indicators

- **Success rate:** based on eligible probes; controller errors are excluded.
- **P50/P95:** latency distribution of successful samples only.
- **Successive-sample jitter:** mean absolute difference between consecutive successful delays, not an independent line-jitter instrument.

Small samples are only a hint. HTTP/HTTPS results are not separate TCP or TLS diagnostics and do not establish UDP or bandwidth availability.

## Privacy and further tests

Disable recording or clear all nodes' history as needed. Latency history lasts up to seven days with a capacity limit. Storage failures may leave some records in the current session only; inspect notifications.

For explicit TCP/HTTPS/UDP checks or effective throughput, select [component tests](/guide/node-tests) and start them yourself.
