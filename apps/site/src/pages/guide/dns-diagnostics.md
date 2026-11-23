---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: dns-diagnostics
---

## Compare two resolution paths

Open **Monitor → Diagnostics → DNS paths**, enter a hostname, select `A` or `AAAA` and run the check. The page shows system resolution and the core DNS interface result, status and observed duration separately.

Each explicit check sends the hostname to the configured system/core resolvers. There are no automatic background queries. The last five results remain in this session and can be copied or cleared; they are not uploaded as persistent diagnostics.

## Choose the next check

| Result | Next step |
| --- | --- |
| System succeeds, core fails | Inspect core upstreams, rules and DNS service |
| Core succeeds, system fails | Inspect system DNS and local network restrictions |
| Both fail | Check spelling and connectivity before the DNS service |
| Both succeed | Continue with proxy connectivity and the destination; this does not exclude every DNS issue |
| Core stopped or unobserved | Start the core and retry; unobserved is not failed resolution |

## Observation limits

The core DNS API need not match every application's resolution route. A hosts entry may avoid a system DNS query. Missing upstream, cache-hit or internal-duration facts remain unknown. Observed duration includes controller round trips.

Recheck after changing profile, core, DNS mode or record family. [DNS settings](/guide/dns) and diagnostic queries are separate actions.
