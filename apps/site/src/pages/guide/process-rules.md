---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: process-rules
---

## Route by application

Open **Configuration → Process rules → Add rule**. Enter the actual process name, such as `Telegram` or `curl`, and choose direct, block or a proxy policy. An identified process can also be added from a connection row's action menu.

Saving applies the rule and reloads the core, which may briefly affect existing connections. Check a newly created connection's matched rule and outbound chain.

## Process identity matters

Process rules precede user rules, independent rule sets and subscription rules. They depend on the core being able to identify the process that created a connection. **Unknown process** means no usable identity is available for that connection; it is not a guessed application name.

A profile switch may remove a policy group. Rules with unavailable targets remain stored but temporarily inactive. Check the active profile and available targets. A subscription node name is not a system process name.

Use [domain and IP rules](/guide/user-rules) for website-based routing. After migrating to another device, verify process names and policies again.
