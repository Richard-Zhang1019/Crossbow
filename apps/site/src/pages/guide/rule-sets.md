---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: rule-sets
---

## Add a remote rule set

Open **Configuration → Rule sets**. Enter a name, URL, match behavior, target policy and update interval. Saving downloads and validates the list first. Supported content is the domain, ipcidr and classical text subset in YAML `payload` or plain text. Unsupported entries are rejected, not silently dropped.

Each rule set is limited to 2 MiB and 50,000 entries. Binary MRS/SRS is not supported. A routing rule set is not a node subscription.

## Order, cache and failures

In rule mode, process and user rules match first, then independent rule sets in list order, followed by subscription rules. You can reorder, enable, disable or update a list manually.

A failed update keeps the validated local cache and reports its cause. Missing targets retain their configuration but are skipped. Disabling stops injection and scheduled refresh; it is not a permanent offline-cache mode.

Check format, URL and target before retrying. Remove URL secrets before sharing a failure. Validated cache and metadata can travel in [portable package v2](/guide/migration).
