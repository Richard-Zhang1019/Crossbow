---
layout: ../../layouts/DocsLayout.astro
lang: en
slug: lan-sharing
---

## Share with another device

Enable **Settings → Network & core → Allow LAN connections**. Find this computer's address and mixed listening port. Set the other device's HTTP/SOCKS proxy to that address and port. **Do not use `127.0.0.1` on the other device.**

If multiple adapters appear, choose an address reachable from that device. Refresh or copy addresses beside **Local addresses**. After changing the listening port, update the client device too.

## Check a failed connection

1. The core and intended profile are running.
2. Both devices can reach each other; the router is not isolating clients.
3. The system firewall permits the inbound port.
4. Protocol, address and port are correct.

LAN listening makes the local proxy reachable by network peers. Enable it only on a trusted network. A displayed address does not prove firewall access or automatically route all traffic from another device. Disable sharing when finished.
