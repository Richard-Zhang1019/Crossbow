---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: user-rules
---

## Set a policy for a domain or IP

Open **Configuration → User rules**. You can also prefill a target from **Monitor → Connections → Connection details → Create domain/IP rule**, then review and save it.

| Match | Scope |
| --- | --- |
| DOMAIN | Exact hostname |
| DOMAIN-SUFFIX | The domain and its subdomains |
| IP-CIDR | Destination IP network, without triggering a DNS lookup |

Choose direct, block or an available policy target. Duplicate conditions are rejected. Save and verify the matched rule in connection details.

## Order and unavailable targets

Rules apply in rule mode: **process rules → user rules, top to bottom → independent rule sets → subscription rules**. Reorder or disable entries as needed. Broad suffixes or networks may shadow later rules; static hints are not proof of a core match.

If a subscription change removes a target, the rule is retained but not injected. It never silently becomes DIRECT. Correct the target and inspect [connections](/guide/connections).

Portable package v2 **does not include domain/IP user rules**. Do not assume they moved with the package; see [configuration migration](/guide/migration).
