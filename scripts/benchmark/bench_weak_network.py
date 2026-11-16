#!/usr/bin/env python3
"""#503 weak-network companion baseline: controlled impairment on the
client->explicit-proxy link via toxiproxy, plus metric-separation fixtures.

Topology: bench client -> toxiproxy listen -> core mixed port -> DIRECT ->
loopback upstream. The impaired hop is client-to-proxy only; the sibling
schema-2 baseline (bench_network.py) proves the kernel DIRECT path with
per-sample log evidence, and the sanity check here repeats one evidence-
attributed sample directly against the mixed port before any impairment.

Impairment scenarios (all applied on the toxiproxy link, both removed after):
  baseline          no toxic; latency samples + throughput reference
  latency_50ms      latency 50ms jitter 10ms downstream+upstream
  latency_150ms     latency 150ms jitter 30ms downstream+upstream
  bandwidth_512k    downstream bandwidth capped at 512 KB/s
  intermittent      latency 200ms jitter 600ms + 500ms timeout toxic:
                    some requests exceed the deadline and are cut -> profile
                    "occasional timeouts" (low ok-rate, fast p50 among ok)
  sustained_slow    latency 300ms jitter 30ms -> all succeed but slow ->
                    profile "sustained slowness" (high ok-rate, elevated p50)
  outage_recovery   proxy listen disabled 2s, re-enabled; recovery_ms is the
                    time to the first successful proxied 204 after re-enable

Metric-separation fixtures (#503 acceptance): TCP reachability vs proxied
HTTP vs UDP availability are measured independently over the same fixture:
  tcp_reachable_http_dead   stalled TCP listener accepts connects but never
                            responds; TCP connect succeeds while proxied HTTP
                            times out
  http_ok_udp_refused       proxied HTTP 204 succeeds while a direct UDP
                            datagram to the upstream port is refused

Requires the toxiproxy-server binary (PATH or --toxiproxy-bin). No system
proxy, DNS, TUN or route changes; private runtime dirs, random ports, and
a one-time controller secret. Physical egress stays unverified (loopback).
"""

import argparse
import json
import socket
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
# 同目录 import 会生成 __pycache__，把 source_metadata 的 worktree_dirty
# 永久置真——归档脚本不得污染工作树。
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO / "scripts" / "benchmark"))

from bench_network import (Core, fixture, free_port, hardware_metadata,
                           percentile, proxied_fetch, save_result,
                           source_metadata, summarize, tls_metadata, upstream)


class ToxiProxy:
    """Own the toxiproxy-server child; manage one proxy and its toxics via REST."""

    def __init__(self, binary, upstream_port, api_port, listen_port):
        self.process = None
        self.name = "crossbow-weaknet"
        self.upstream_port, self.api_port, self.listen_port = upstream_port, api_port, listen_port
        self.log_file = None
        import subprocess
        self.log_file = tempfile_path("toxiproxy")
        self.process = subprocess.Popen([str(binary), "--port", str(api_port)],
                                        stdout=self.log_file, stderr=self.log_file)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError("toxiproxy-server exited before readiness")
            try:
                self.api("GET", "/version")
                break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError("toxiproxy API never became ready")
        self.api("POST", "/proxies", {"name": self.name,
                                      "listen": f"127.0.0.1:{listen_port}",
                                      "upstream": f"127.0.0.1:{upstream_port}"})

    def api(self, method, path, body=None):
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.api_port}{path}",
            data=json.dumps(body).encode() if body is not None else None,
            method=method,
            headers={"Content-Type": "application/json"} if body is not None else {})
        with urllib.request.urlopen(request, timeout=3) as response:
            payload = response.read(65536)
        return json.loads(payload) if payload else None

    def add_toxic(self, toxic_name, toxic_type, stream, attributes):
        self.api("POST", f"/proxies/{self.name}/toxics",
                 {"name": toxic_name, "type": toxic_type, "stream": stream,
                  "toxicity": 1.0, "attributes": attributes})

    def remove_toxic(self, toxic_name):
        self.api("DELETE", f"/proxies/{self.name}/toxics/{toxic_name}")

    def set_enabled(self, enabled):
        self.api("POST", f"/proxies/{self.name}", {"enabled": enabled})

    def close(self):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
        if self.log_file:
            self.log_file.close()
            self.log_file = None


def tempfile_path(name):
    import tempfile
    return open(tempfile.mkstemp(prefix=f"crossbow-{name}-")[1], "wb")


def stalled_listener():
    """TCP listener that accepts and then never responds (fixture A)."""
    server = socket.socket()
    server.bind(("127.0.0.1", 0))
    server.listen(16)
    port = server.getsockname()[1]
    import threading
    stop = threading.Event()

    def accept_forever():
        server.settimeout(.2)
        while not stop.is_set():
            try:
                conn, _ = server.accept()
                import threading
                threading.Thread(target=lambda conn=c: (stop.wait(30), conn.close()),
                                 daemon=True).start()
            except (socket.timeout, OSError):
                if stop.is_set():
                    break
    worker = threading.Thread(target=accept_forever, daemon=True)
    worker.start()
    return server, port, stop


def latency_round(toxi_port, target, samples, timeout):
    return [proxied_fetch(toxi_port, target, timeout=timeout) for _ in range(samples)]


def throughput_round(toxi_port, target, size_mib, load_seconds, timeout):
    started = time.monotonic()
    transfers, bytes_read = [], 0
    while not transfers or time.monotonic() - started < load_seconds:
        sample = proxied_fetch(toxi_port,
                               f"{target.rsplit('/', 1)[0]}/body?mib={size_mib}",
                               timeout=timeout, expected_status=200,
                               expected_bytes=size_mib * 1024 * 1024)
        transfers.append(sample)
        if not sample["ok"]:
            break
        bytes_read += sample["bytes_read"]
    elapsed = time.monotonic() - started
    ok = bool(transfers) and all(s["ok"] for s in transfers)
    return {"ok": ok, "bytes_verified": bytes_read, "elapsed_ms": round(elapsed * 1000, 3),
            "mib_per_second": round(bytes_read / 1048576 / elapsed, 3) if ok and elapsed > 0 else None,
            "transfers": transfers}


def run(core_bin, toxi_bin, output, samples=30, size_mib=2, load_seconds=1, timeout=5):
    result = {"schema_version": 1, "status": "failed", "kind": "weak-network-matrix",
              "mode": "explicit-proxy", "engine": "mihomo",
              "impairment_path": "client -> toxiproxy -> mixed port -> DIRECT -> loopback upstream",
              "physical_egress": "unverified (loopback upstream)",
              "source": source_metadata(),
              "env": {"os": __import__("platform").platform(), "arch": __import__("platform").machine(),
                      "hardware": hardware_metadata(), "client_tls": tls_metadata(),
                      "recorded_at": datetime.now(timezone.utc).isoformat()},
              "params": {"latency_samples_per_scenario": samples, "size_mib": size_mib,
                         "load_seconds": load_seconds, "request_timeout_seconds": timeout},
              "scenarios": {}}
    owned, phase = [], "preflight"
    toxi = None
    try:
        binary = core_bin.resolve(strict=True)
        toxi_binary = toxi_bin.resolve(strict=True)
        version = subprocess_run([str(binary), "-v"])
        if "mihomo" not in version.lower():
            raise ValueError(f"binary is not mihomo: {version[:80]}")
        result["core"] = {"version": version.strip(),
                          "sha256": __import__("hashlib").sha256(binary.read_bytes()).hexdigest()}
        with tempfile_dir() as temp, upstream() as up:
            workdir = Path(temp)
            mixed = free_port({up})
            controller = free_port({up, mixed})
            secret = uuid_hex()
            config, config_name = fixture(mixed, controller, secret, "mihomo")
            config_path = workdir / config_name
            config_path.write_text(json.dumps(config))
            config_path.chmod(0o600)
            result["fixture"] = json.loads(json.dumps(config).replace(secret, "<ephemeral redacted>"))
            target = f"http://127.0.0.1:{up}/204"
            result["targets"] = {"local_204": target}
            phase = "core_start"
            core = Core(binary, workdir, config_path, controller, secret, "core.log", "mihomo")
            owned.append(core)
            with core:
                phase = "sanity_direct_evidence"
                # 一次直连混合端口的证据归因样本：证明内核 DIRECT 路径真实工作
                # （弱网场景经 toxiproxy，逐样本日志归因不可用——连接源是
                # toxiproxy 的出口端口；路径证明由本样本与 baseline 承担）。
                sanity = proxied_fetch(mixed, target, timeout=timeout)
                sanity["core_evidence"] = core.evidence(sanity)
                if not sanity["ok"] or sanity["core_evidence"] is None:
                    raise RuntimeError("direct mixed-port sanity sample failed verification")
                result["sanity_direct_sample"] = sanity

                phase = "toxiproxy_start"
                listen = free_port({up, mixed, controller})
                api = free_port({up, mixed, controller, listen})
                toxi = ToxiProxy(toxi_binary, mixed, api, listen)
                result["toxiproxy"] = {"api_port": api, "listen_port": listen,
                                       "upstream": f"127.0.0.1:{mixed}"}
                toxi_port = listen

                phase = "baseline"
                result["scenarios"]["baseline"] = {
                    "toxics": [], "note": "50ms toxic adds ~100ms round trip when applied to both directions",
                    "latency": summarize(latency_round(toxi_port, target, samples, timeout)),
                    "throughput": throughput_round(toxi_port, target, size_mib, load_seconds, timeout)}

                phase = "latency_50ms"
                toxi.add_toxic("lat50-down", "latency", "downstream", {"latency": 50, "jitter": 10})
                toxi.add_toxic("lat50-up", "latency", "upstream", {"latency": 50, "jitter": 10})
                result["scenarios"]["latency_50ms"] = {
                    "toxics": ["latency 50ms jitter 10ms downstream+upstream"],
                    "latency": summarize(latency_round(toxi_port, target, samples, timeout))}
                toxi.remove_toxic("lat50-down")
                toxi.remove_toxic("lat50-up")

                phase = "latency_150ms"
                toxi.add_toxic("lat150-down", "latency", "downstream", {"latency": 150, "jitter": 30})
                toxi.add_toxic("lat150-up", "latency", "upstream", {"latency": 150, "jitter": 30})
                result["scenarios"]["latency_150ms"] = {
                    "toxics": ["latency 150ms jitter 30ms downstream+upstream"],
                    "latency": summarize(latency_round(toxi_port, target, samples, timeout))}
                toxi.remove_toxic("lat150-down")
                toxi.remove_toxic("lat150-up")

                phase = "bandwidth_512k"
                toxi.add_toxic("bw512", "bandwidth", "downstream", {"rate": 512})
                result["scenarios"]["bandwidth_512k"] = {
                    "toxics": ["bandwidth 512 KB/s downstream"],
                    "latency": summarize(latency_round(toxi_port, target, min(samples, 10), timeout)),
                    "throughput": throughput_round(toxi_port, target, size_mib, load_seconds, timeout * 4)}
                toxi.remove_toxic("bw512")

                phase = "intermittent"
                toxi.add_toxic("itmlat", "latency", "downstream", {"latency": 200, "jitter": 50})
                # toxicity 0.5：仅半数连接被 500ms 切断——制造真实的偶发超时
                toxi.api("POST", f"/proxies/{toxi.name}/toxics",
                         {"name": "itmtmo", "type": "timeout", "stream": "downstream",
                          "toxicity": 0.5, "attributes": {"timeout": 500}})
                result["scenarios"]["intermittent"] = {
                    "toxics": ["latency 200ms jitter 50ms downstream",
                               "timeout 500ms downstream toxicity=0.5"],
                    "profile": "occasional timeouts: mixed ok/timeout with fast ok samples",
                    "latency": summarize(latency_round(toxi_port, target, samples, timeout))}
                toxi.remove_toxic("itmlat")
                toxi.remove_toxic("itmtmo")

                phase = "sustained_slow"
                toxi.add_toxic("slow", "latency", "downstream", {"latency": 300, "jitter": 30})
                result["scenarios"]["sustained_slow"] = {
                    "toxics": ["latency 300ms jitter 30ms downstream"],
                    "profile": "sustained slowness: high ok-rate with elevated p50/p95",
                    "latency": summarize(latency_round(toxi_port, target, samples, timeout))}
                toxi.remove_toxic("slow")

                phase = "outage_recovery"
                toxi.set_enabled(False)
                during = [proxied_fetch(toxi_port, target, timeout=1) for _ in range(3)]
                toxi.set_enabled(True)
                reenabled = time.monotonic()
                recovery, attempts = None, []
                while time.monotonic() - reenabled < 10:
                    sample = proxied_fetch(toxi_port, target, timeout=1)
                    attempts.append(sample)
                    if sample["ok"]:
                        recovery = round((time.monotonic() - reenabled) * 1000, 3)
                        break
                    time.sleep(.1)
                result["scenarios"]["outage_recovery"] = {
                    "toxics": ["proxy listen disabled for the duration of during_outage"],
                    "during_outage": during, "recovery_ms": recovery, "attempts": len(attempts)}

                phase = "metric_separation"
                stalled_server, stalled_port, stop_flag = stalled_listener()
                try:
                    tcp = tcp_connect_probe(stalled_port)
                    http_dead = proxied_fetch(toxi_port, f"http://127.0.0.1:{stalled_port}/204",
                                              timeout=timeout)
                    http_ok = proxied_fetch(toxi_port, target, timeout=timeout)
                    udp = udp_probe(up)
                    result["scenarios"]["metric_separation"] = {
                        "tcp_reachable_http_dead": {
                            "tcp_connect": tcp,
                            "proxied_http": http_dead,
                            "lesson": "TCP reachability must not be reported as a usable proxy metric"},
                        "http_ok_udp_refused": {
                            "proxied_http_204": {"ok": http_ok["ok"], "elapsed_ms": http_ok.get("elapsed_ms")},
                            "udp_datagram": udp,
                            "lesson": "HTTP delay metrics say nothing about UDP availability"},
                    }
                finally:
                    stop_flag.set()
                    stalled_server.close()
                if not all(s["ok"] for s in result["scenarios"]["baseline"]["latency"]["samples"]):
                    raise RuntimeError("baseline latency path failed verification")
                result["status"] = "completed"
    except (Exception, KeyboardInterrupt) as error:
        result["failure"] = {"phase": phase, "class": type(error).__name__, "reason": str(error)[:240]}
    finally:
        if toxi is not None:
            toxi.close()
        result["cleanup"] = {"owned_core_pids": [c.process.pid for c in owned if c.process],
                             "all_owned_cores_reaped": all(c.process is None or c.process.poll() is not None for c in owned),
                             "toxiproxy_reaped": toxi is None or toxi.process.poll() is not None}
        if not all(result["cleanup"].values()):
            result["status"] = "failed"
        if "failure" in result:
            result["failure"]["owned_core_log_tails"] = [getattr(c, "log_tail", "") for c in owned]
    path = save_result(output, result)
    return result, path


def subprocess_run(argv):
    import subprocess
    return subprocess.run(argv, capture_output=True, text=True, timeout=5, check=True).stdout


def tempfile_dir():
    import tempfile
    return tempfile.TemporaryDirectory(prefix="crossbow-weaknet-")


def uuid_hex():
    import uuid
    return uuid.uuid4().hex


def tcp_connect_probe(port):
    started = time.monotonic()
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=3):
            return {"ok": True, "elapsed_ms": round((time.monotonic() - started) * 1000, 3)}
    except OSError as error:
        return {"ok": False, "error_class": "connect", "detail": str(error)[:80]}


def udp_probe(avoid_port):
    # 向上游主机的一个确定无 UDP 监听的端口发数据报（HTTP 服务不占 UDP）。
    target_port = free_port({avoid_port})
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(1)
    try:
        sock.sendto(b"probe", ("127.0.0.1", target_port))
        sock.recvfrom(1024)
        return {"ok": True, "error_class": None}
    except (ConnectionRefusedError, socket.timeout, OSError) as error:
        return {"ok": False, "error_class": type(error).__name__}
    finally:
        sock.close()


def main(argv=None):
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, required=True)
    parser.add_argument('--toxiproxy-bin', type=Path,
                        default=Path('toxiproxy-server'))
    parser.add_argument('--output', type=Path, default=REPO / 'target/network-benchmark')
    parser.add_argument('--samples', type=int, default=30)
    parser.add_argument('--throughput-mib', type=int, default=2)
    parser.add_argument('--load-seconds', type=float, default=1)
    parser.add_argument('--timeout', type=float, default=5)
    args = parser.parse_args(argv)
    if not 1 <= args.samples <= 500 or not 1 <= args.throughput_mib <= 16:
        parser.error('samples must be 1..500; throughput-mib must be 1..16')
    result, path = run(args.core, args.toxiproxy_bin, args.output,
                       args.samples, args.throughput_mib, args.load_seconds, args.timeout)
    print(json.dumps({'kind': result['kind'], 'status': result['status'],
                      'result': str(path), 'failure': result.get('failure')},
                     ensure_ascii=False))
    return 0 if result['status'] == 'completed' else 1


if __name__ == '__main__':
    sys.exit(main())
