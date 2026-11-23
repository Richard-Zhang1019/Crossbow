---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: nodes
---

## Choose the outbound

Check the active profile in the top-right switcher, then open **Nodes**. Choose a policy group on the left. Search, filter by region or enable **Reachable only**. Click a card to change that group's selection; the top-right check marks the selected node.

The lightning button in a card's bottom row tests that node without selecting it. The header's **Test latency** action tests the current group and can be cancelled.

## Automatic versus pinned selection

An automatic group follows the interval and tolerance in its configuration. Clicking a node may pin it. Use the provided restore-automatic action to return to automatic selection. A measurement alone is not a request to switch routes.

## Read latency results

Untested, timeout, request failure and controller error are different states. Do not interpret an untested node as unusable. Targets and core measurement policies differ; one latency value does not describe long-term stability or bandwidth.

Use [Quality history](/guide/node-quality) for success rates and distributions, or explicitly run [component tests](/guide/node-tests). If no profile is active or the core is missing or stopped, resolve those prerequisites first.
