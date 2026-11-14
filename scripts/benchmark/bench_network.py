#!/usr/bin/env python3
"""#492：DNS、协议与 TUN 网络性能基线。

用脱敏夹具（DIRECT-only，固定 204 端点）驱动真实内核，测量：
冷启动/热缓存首请求耗时、延迟 P50/P95、成功率、回环吞吐、内核 CPU/内存、
DNS 解析基线、内核重启恢复时间。所有请求显式走混合端口（不改系统设置），
出口与目的端全程记录。

模式：
  sysproxy  —— 显式经混合端口测速（默认；本机可跑，不改系统配置）
  tun       —— 生成 TUN 夹具（对齐 runtime.rs 注入形状）并打印受控环境
               命令；需要 sudo 建 utun/路由，默认拒绝在本机直接执行，
               传入 --i-know-this-touches-my-network 才放行。

输出：<output>/results.json + stdout 摘要。指标定义见 docs/benchmarks/README.md。
"""
import argparse
import http.server
import json
import os
import platform
from pathlib import Path
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

REPO = Path(__file__).resolve().parents[2]
PROBE_TARGETS = [
    "http://www.gstatic.com/generate_204",
    "http://cp.cloudflare.com/generate_204",
]
DNS_TARGET = "www.gstatic.com"


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def now_ms():
    return time.monotonic_ns() // 1_000_000


class LocalUpstream(http.server.BaseHTTPRequestHandler):
    """回环上游：/204 探针 + /body 可控大小吞吐体。回环隔离 WAN 抖动，
    内核吞吐开销（sysproxy vs TUN）因此在受控条件下可比较。"""

    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path == "/204":
            self.send_response(204)
            self.end_headers()
        elif self.path.startswith("/body"):
            size = int(self.path.split("mb=")[-1] or 8) * 1024 * 1024
            self.send_response(200)
            self.send_header("Content-Length", str(size))
            self.end_headers()
            chunk = b"\0" * (1024 * 1024)
            for _ in range(size // (1024 * 1024)):
                self.wfile.write(chunk)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, *_args):
        pass


def start_upstream():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalUpstream)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, server.server_address[1]


def proxied_fetch(mixed_port, url, timeout=5):
    """显式经混合端口请求 url（等效系统代理路径，但不写系统设置）。
    url 可以是回环上游（受控体）或公网端点（记录真实路径样本）。"""
    started = time.monotonic_ns()
    proxy = urllib.request.ProxyHandler(
        {"http": f"http://127.0.0.1:{mixed_port}", "https": f"http://127.0.0.1:{mixed_port}"}
    )
    opener = urllib.request.build_opener(proxy)
    try:
        with opener.open(url, timeout=timeout) as resp:
            code = resp.status
            resp.read()
        ms = (time.monotonic_ns() - started) // 1_000_000
        return {"ok": 200 <= code < 300, "http_status": code, "ms": ms}
    except Exception as error:  # noqa: BLE001 —— 归类为失败并截断错误文本
        return {"ok": False, "error": str(error)[:120],
                "ms": (time.monotonic_ns() - started) // 1_000_000}


def wait_core_ready(controller, secret, timeout=15):
    started = time.monotonic()
    while time.monotonic() - started < timeout:
        try:
            req = urllib.request.Request(
                f"http://127.0.0.1:{controller}/version",
                headers={"Authorization": f"Bearer {secret}"},
            )
            with urllib.request.urlopen(req, timeout=1) as resp:
                if resp.status == 200:
                    return (time.monotonic() - started) * 1000
        except Exception:  # noqa: BLE001
            time.sleep(0.1)
    raise RuntimeError("core controller never became ready")


def spawn_core(core_bin, workdir, config_name, mixed, controller, secret):
    process = subprocess.Popen(
        [str(core_bin), "-d", str(workdir), "-f", str(workdir / config_name)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    ready_ms = wait_core_ready(controller, secret)
    return process, ready_ms


def core_stats(pid):
    out = subprocess.run(
        ["ps", "-p", str(pid), "-o", "%cpu=", "-o", "rss="],
        capture_output=True, text=True,
    ).stdout.strip()
    if not out:
        return None
    parts = out.split()
    return {"cpu_percent": float(parts[0]), "rss_kb": int(parts[1])}


def percentile(samples, p):
    ordered = sorted(samples)
    index = min(len(ordered) - 1, round(len(ordered) * p / 100))
    return ordered[index]


def dns_baseline(target, count=10):
    samples = []
    for _ in range(count):
        started = time.monotonic_ns()
        socket.getaddrinfo(target, 443, proto=socket.IPPROTO_TCP)
        samples.append((time.monotonic_ns() - started) // 1_000_000)
    return {"count": count, "p50_ms": percentile(samples, 50),
            "p95_ms": percentile(samples, 95)}


def fixture(mode, mixed, controller, secret):
    """脱敏夹具：DIRECT-only、无节点无订阅；TUN 形状对齐 runtime.rs 注入。"""
    dns = """dns:
  enable: true
  nameserver:
    - 223.5.5.5
    - 119.29.29.29
"""
    if mode == "sysproxy":
        return f"""mixed-port: {mixed}
external-controller: 127.0.0.1:{controller}
secret: {secret}
{dns}rules:
  - MATCH,DIRECT
"""
    # TUN：对齐 runtime.rs 的注入形状（gvisor、native DNS、fake-ip、
    # 私网豁免、嗅探），供受控环境复跑。
    return f"""mixed-port: {mixed}
external-controller: 127.0.0.1:{controller}
secret: {secret}
{dns}tun:
  enable: true
  stack: gvisor
  auto-route: true
  auto-redirect: true
  auto-detect-interface: true
  dns-mode: native
  dns-hijack:
    - any:53
    - tcp://any:53
  route-exclude-address:
    - 192.168.0.0/16
    - 100.64.0.0/10
    - 10.0.0.0/8
    - 172.16.0.0/12
sniffer:
  enable: true
  override-destination: true
  force-dns-mapping: true
  parse-pure-ip: true
  TLS:
    ports: [443, 8443]
  HTTP:
    ports: [80, 8080-8880]
    override-destination: true
dns:
  enable: true
  enhanced-mode: fake-ip
rules:
  - MATCH,DIRECT
"""


def measure_round(process, mixed, upstream_port, samples=30, throughput_mb=8):
    """一轮完整测量：延迟样本、公网样本、回环吞吐、内核 CPU/内存。"""
    round_result = {"latency_samples": 0, "latency_ok": 0, "latencies_ms": [],
                    "p50_ms": None, "p95_ms": None,
                    "public_p50_ms": None, "public_ok": 0, "public_samples": 10,
                    "throughput_mb_s": None, "cpu_avg_percent": None,
                    "cpu_peak_percent": None, "rss_peak_kb": None}

    stop = threading.Event()
    cpu_samples = []

    def sampler():
        while not stop.is_set():
            stats = core_stats(process.pid)
            if stats:
                cpu_samples.append(stats)
            time.sleep(0.5)

    sampler_thread = threading.Thread(target=sampler, daemon=True)
    sampler_thread.start()

    probe_url = f"http://127.0.0.1:{upstream_port}/204"
    latencies = []
    ok_count = 0
    for _ in range(samples):
        result = proxied_fetch(mixed, probe_url)
        latencies.append(result["ms"])
        ok_count += result["ok"]
    round_result["latency_samples"] = samples
    round_result["latency_ok"] = ok_count
    round_result["latencies_ms"] = latencies
    round_result["p50_ms"] = percentile(latencies, 50)
    round_result["p95_ms"] = percentile(latencies, 95)

    public = [proxied_fetch(mixed, PROBE_TARGETS[0]) for _ in range(10)]
    public_ok = [entry["ms"] for entry in public if entry["ok"]]
    round_result["public_ok"] = len(public_ok)
    round_result["public_p50_ms"] = percentile(public_ok, 50) if public_ok else None

    started = time.monotonic()
    download = proxied_fetch(
        mixed, f"http://127.0.0.1:{upstream_port}/body?mb={throughput_mb}", timeout=60)
    elapsed = time.monotonic() - started
    if download["ok"] and elapsed > 0:
        round_result["throughput_mb_s"] = round(throughput_mb / elapsed, 1)

    stop.set()
    sampler_thread.join(timeout=2)
    if cpu_samples:
        round_result["cpu_avg_percent"] = round(
            statistics.mean(entry["cpu_percent"] for entry in cpu_samples), 1)
        round_result["cpu_peak_percent"] = max(
            entry["cpu_percent"] for entry in cpu_samples)
        round_result["rss_peak_kb"] = max(entry["rss_kb"] for entry in cpu_samples)
    return round_result


def run(mode, core_bin, output, samples, throughput_mb):
    mixed, controller, upstream_port = free_port(), free_port(), free_port()
    while controller in (mixed, upstream_port):
        controller = free_port()
    secret = "crossbow-benchmark"
    server, upstream_port = start_upstream()

    workdir = Path(tempfile.mkdtemp(prefix="crossbow-bench-"))
    (workdir / f"{mode}.yaml").write_text(fixture(mode, mixed, controller, secret))

    core_version = subprocess.run([str(core_bin), "-v"], capture_output=True,
                                  text=True).stdout.splitlines()[0].strip()
    results = {"mode": mode, "core": core_version, "egress": "DIRECT",
               "env": {"os": platform.platform(), "arch": platform.machine(),
                       "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z")},
               "targets": {"probe": PROBE_TARGETS, "dns": DNS_TARGET,
                           "upstream": f"127.0.0.1:{upstream_port}"},
               "params": {"latency_samples": samples,
                          "throughput_mb": throughput_mb}}

    # 冷启动首请求：进程拉起 → 控制器就绪 → 第一个经代理 204 成功。
    started = time.monotonic_ns()
    process, cold_wait = spawn_core(core_bin, workdir, f"{mode}.yaml",
                                    mixed, controller, secret)
    probe_url = f"http://127.0.0.1:{upstream_port}/204"
    while True:
        outcome = proxied_fetch(mixed, probe_url, timeout=3)
        if outcome["ok"]:
            break
        if time.monotonic_ns() - started > 20_000_000_000:
            raise RuntimeError("cold first request never succeeded")
        time.sleep(0.05)
    results["cold_first_request_ms"] = {
        "controller_ready_ms": round(cold_wait, 1),
        "first_proxied_ok_ms": (time.monotonic_ns() - started) // 1_000_000,
    }

    results["rounds"] = []
    for index in range(2):
        results["rounds"].append(measure_round(process, mixed, upstream_port,
                                               samples, throughput_mb))
    results["dns_direct"] = dns_baseline(DNS_TARGET)

    # 恢复时间：杀内核 → 重启 → 首个经代理 204 成功（含拉起时间）。
    process.terminate()
    process.wait(timeout=8)
    started = time.monotonic_ns()
    restarted, _ = spawn_core(core_bin, workdir, f"{mode}.yaml", mixed,
                              controller, secret)
    while True:
        outcome = proxied_fetch(mixed, probe_url, timeout=3)
        if outcome["ok"]:
            break
        if time.monotonic_ns() - started > 20_000_000_000:
            raise RuntimeError("recovery request never succeeded")
        time.sleep(0.05)
    results["recovery_ms"] = (time.monotonic_ns() - started) // 1_000_000
    restarted.terminate()
    restarted.wait(timeout=8)
    server.shutdown()

    output.mkdir(parents=True, exist_ok=True)
    (output / f"{mode}-results.json").write_text(json.dumps(results, indent=1))
    print(json.dumps(results, ensure_ascii=False, indent=1))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path,
                        default=REPO / "apps/desktop/src-tauri/binaries/mihomo-aarch64-apple-darwin")
    parser.add_argument("--mode", choices=["sysproxy", "tun"], default="sysproxy")
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--throughput-mb", type=int, default=8)
    parser.add_argument("--output", type=Path,
                        default=REPO / "docs/benchmarks")
    parser.add_argument("--i-know-this-touches-my-network", action="store_true",
                        help="TUN 模式会建 utun 并改路由；仅在受控设备上传入本开关")
    args = parser.parse_args()

    if not args.core.exists():
        raise SystemExit(f"core binary missing: {args.core}")
    if args.mode == "tun" and not args.i_know_this_touches_my_network:
        fixture_path = Path(tempfile.mkdtemp(prefix="crossbow-bench-tun-")) / "tun.yaml"
        fixture_path.write_text(fixture("tun", free_port(), free_port(), "crossbow-benchmark"))
        print(f"""TUN 基线需要 sudo 建 utun/改路由——请在受控设备/虚拟机执行：

  sudo {args.core} -d <workdir> -f {fixture_path}

夹具已生成（对齐 runtime.rs 注入形状）。加 --i-know-this-touches-my-network
可在本机强制执行。测量通过系统路由命中 TUN 后直接请求 204/吞吐端点。""")
        return
    run(args.mode, args.core, args.output, args.samples, args.throughput_mb)


if __name__ == "__main__":
    main()
