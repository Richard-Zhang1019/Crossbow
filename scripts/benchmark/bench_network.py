#!/usr/bin/env python3
"""Measure an isolated mihomo explicit HTTP proxy, never system proxy or TUN.

Loopback probes require both the proxy socket and the owned core's DIRECT log.
Public HTTPS and system-resolver samples are optional and kept separate by URL.
No subscription, user configuration, privilege elevation or route writes.
"""
import argparse
from collections import Counter
from contextlib import ExitStack, contextmanager
from datetime import datetime, timezone
import hashlib
import http.client
import http.server
import json
import math
import os
from pathlib import Path
import platform
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import parse_qs, urlsplit
import uuid

REPO = Path(__file__).resolve().parents[2]
PROBE_TARGETS = ("https://www.gstatic.com/generate_204", "https://cp.cloudflare.com/generate_204")
DNS_TARGET = "www.gstatic.com"
MIB = 1024 * 1024


def free_port(exclude=()):
    while True:
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        if port not in exclude:
            return port


def percentile(values, p):
    """Nearest-rank percentile; empty populations have no percentile."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, min(len(ordered) - 1, math.ceil(len(ordered) * p / 100) - 1))]


def summarize(samples):
    valid = [sample["elapsed_ms"] for sample in samples if sample["ok"]]
    return {"count": len(samples), "ok_count": len(valid),
            "percentile_method": "nearest_rank_successes_only",
            "success_rate": len(valid) / len(samples) if samples else None,
            "p50_ms": percentile(valid, 50), "p95_ms": percentile(valid, 95),
            "failures": dict(Counter(s["error_class"] for s in samples if not s["ok"])),
            "samples": samples}


class LocalUpstream(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        url = urlsplit(self.path)
        if url.path == "/204":
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()
        elif url.path == "/body":
            try:
                size = int(parse_qs(url.query)["mib"][0])
                if not 1 <= size <= 64:
                    raise ValueError("body size out of range")
            except (KeyError, ValueError):
                self.send_error(400)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(size * MIB))
            self.end_headers()
            try:
                for _ in range(size):
                    self.wfile.write(b"\0" * MIB)
            except (BrokenPipeError, ConnectionResetError):
                pass  # A cancelled local client cannot be counted as a successful transfer.
        else:
            self.send_error(404)

    def log_message(self, *_args):
        pass


@contextmanager
def upstream():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), LocalUpstream)
    worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=.05), daemon=True)
    worker.start()
    try:
        yield server.server_address[1]
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)


def proxied_fetch(mixed_port, url, timeout=5, expected_status=204, expected_bytes=None):
    """Force an explicit proxy socket, ignoring NO_PROXY, env and macOS bypass rules.

    HTTP uses absolute-form; HTTPS uses CONNECT with standard certificate checks.
    No redirects. Stream/count bytes rather than buffering the throughput body.
    """
    started = time.monotonic()
    result = {"url": url, "ok": False, "http_status": None, "bytes_read": 0,
              "error_class": None, "proxy_peer": None, "source": None}
    target = urlsplit(url)
    conn = None
    try:
        if target.scheme not in ("http", "https") or not target.hostname or target.username:
            raise ValueError("only unauthenticated HTTP(S) benchmark targets are supported")
        if target.scheme == "https":
            conn = http.client.HTTPSConnection("127.0.0.1", mixed_port, timeout=timeout,
                                               context=ssl.create_default_context())
            conn.set_tunnel(target.hostname, target.port or 443)
            path = target.path or "/"
            if target.query:
                path += "?" + target.query
        else:
            conn = http.client.HTTPConnection("127.0.0.1", mixed_port, timeout=timeout)
            path = url
        conn.request("GET", path, headers={"Connection": "close", "Accept-Encoding": "identity"})
        result["proxy_peer"] = list(conn.sock.getpeername()[:2])
        address, port = conn.sock.getsockname()[:2]
        result["source"] = f"{address}:{port}"
        response = conn.getresponse()
        result["http_status"] = response.status
        if response.status != expected_status:
            result["error_class"] = "http_status"
        else:
            deadline = started + timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("response body deadline")
                if conn.sock:
                    conn.sock.settimeout(remaining)
                chunk = response.read(64 * 1024)
                if not chunk:
                    break
                result["bytes_read"] += len(chunk)
                if expected_bytes is not None and result["bytes_read"] > expected_bytes:
                    break
            if expected_bytes is not None and result["bytes_read"] != expected_bytes:
                result["error_class"] = "body_size"
            elif result["proxy_peer"] != ["127.0.0.1", mixed_port]:
                result["error_class"] = "proxy_peer"
            else:
                result["ok"] = True
    except ssl.SSLCertVerificationError as error:
        result.update(error_class="tls", tls_reason="certificate_verify_failed",
                      tls_verify_code=getattr(error, 'verify_code', None))
    except ssl.SSLError as error:
        result.update(error_class="tls", tls_reason=getattr(error, 'reason', None))
    except (TimeoutError, socket.timeout):
        result["error_class"] = "timeout"
    except http.client.IncompleteRead as error:
        result["bytes_read"] += len(error.partial)
        result["error_class"] = "incomplete_body"
    except (OSError, http.client.HTTPException):
        result["error_class"] = "connect"
    except ValueError:
        result["error_class"] = "invalid_target"
    finally:
        if conn:
            conn.close()
        result["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)
    return result


def fixture(mixed, controller, secret, engine="mihomo"):
    """DIRECT-only loopback fixture per engine. Returns (config_dict, filename)."""
    if engine == "mihomo":
        # JSON is valid YAML; a single mapping prevents duplicate DNS keys.
        return {"mixed-port": mixed, "allow-lan": False, "bind-address": "127.0.0.1",
                "external-controller": f"127.0.0.1:{controller}", "secret": secret,
                "log-level": "info", "ipv6": False, "tun": {"enable": False},
                "dns": {"enable": False}, "profile": {"store-selected": False},
                "rules": ["MATCH,DIRECT"]}, "fixture.yaml"
    if engine == "sing-box":
        return {"log": {"level": "info"},
                "inbounds": [{"type": "mixed", "tag": "mixed-in",
                              "listen": "127.0.0.1", "listen_port": mixed}],
                "outbounds": [{"type": "direct", "tag": "direct"}],
                "route": {"final": "direct"},
                "experimental": {"clash_api": {"external_controller": f"127.0.0.1:{controller}",
                                               "secret": secret}}}, "fixture.json"
    raise ValueError(f"unsupported engine: {engine}")


def wait_core_ready(process, controller, secret, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"owned core exited before readiness: {process.returncode}")
        conn = http.client.HTTPConnection("127.0.0.1", controller, timeout=1)
        try:
            conn.request("GET", "/version", headers={"Authorization": f"Bearer {secret}"})
            response = conn.getresponse()
            if response.status == 200 and json.loads(response.read(4096)).get("version"):
                return
        except (OSError, ValueError, http.client.HTTPException):
            pass
        finally:
            conn.close()
        time.sleep(.02)
    raise RuntimeError("owned core controller never became ready")


ANSI_COLOR = re.compile(r"\x1b\[[0-9;]*m")


class Core:
    """Own and reap the child even if readiness, measurement or restart fails."""
    def __init__(self, binary, workdir, config, controller, secret, log_name, engine="mihomo"):
        if engine == "sing-box":
            self.command = [str(binary), "run", "-c", str(config), "-D", str(workdir)]
        else:
            self.command = [str(binary), "-d", str(workdir), "-f", str(config)]
        self.controller, self.secret = controller, secret
        self.log_path = workdir / log_name
        self.engine = engine
        self.process, self.log = None, None

    def __enter__(self):
        self.log = self.log_path.open("wb")
        self.started = time.monotonic()
        try:
            self.process = subprocess.Popen(self.command, stdout=self.log, stderr=self.log)
            wait_core_ready(self.process, self.controller, self.secret)
            self.ready_ms = (time.monotonic() - self.started) * 1000
            return self
        except BaseException:
            self.close()
            raise

    def close(self):
        try:
            if self.process is not None and self.process.poll() is None:
                try:
                    self.process.terminate()
                except ProcessLookupError:
                    pass
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=3)
        finally:
            if self.log:
                self.log.close()
                self.log = None
            try:
                self.log_tail = self.log_path.read_text(errors="replace")[-1500:].replace(self.secret, "<redacted>")
            except OSError:
                self.log_tail = ""

    def __exit__(self, *_args):
        self.close()

    def evidence(self, sample):
        if not sample["source"]:
            return None
        deadline = time.monotonic() + 1
        while True:
            match = self._evidence_match(sample["source"])
            if match:
                return {"core_outbound": match[0], "core_reported_destination": match[1],
                        "owned_core_pid": self.process.pid}
            if time.monotonic() >= deadline:
                return None
            time.sleep(.01)

    def _evidence_match(self, source):
        text = self.log_path.read_text(errors="replace")
        if self.engine == "mihomo":
            pattern = re.compile(r'\[TCP\] ' + re.escape(source)
                                 + r'(?:\([^\n]*?\))? --> (\S+)[^\n]*?using DIRECT(?=["\s]|$)')
            matches = pattern.findall(text)
            return ("DIRECT", matches[-1]) if matches else None
        # sing-box: ANSI-colored lines keyed by connection id; the direct
        # outbound line shares the id with the inbound-from line.
        text = ANSI_COLOR.sub("", text)
        ids = re.findall(r'\[(\d+)[^\]]*\] inbound/mixed\[mixed-in\]: inbound connection from '
                         + re.escape(source) + r'$', text, flags=re.MULTILINE)
        for connection_id in reversed(ids):
            outbound = re.search(r'\[' + re.escape(connection_id)
                                 + r'[^\]]*\] outbound/direct\[direct\]: outbound connection to (\S+)',
                                 text)
            if outbound:
                return ("direct", outbound.group(1))
        return None


def measured_fetch(core, mixed, url, **kwargs):
    sample = proxied_fetch(mixed, url, **kwargs)
    sample["since_core_start_ms"] = round((time.monotonic() - core.started) * 1000, 3)
    sample["core_evidence"] = core.evidence(sample) if sample["source"] else None
    if sample["ok"] and sample["core_evidence"] is None:
        sample.update(ok=False, error_class="route_evidence_missing")
    return sample


def first_request(core, mixed, url):
    samples = []
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        sample = measured_fetch(core, mixed, url, timeout=2)
        samples.append(sample)
        if sample["ok"]:
            return {"controller_ready_ms": round(core.ready_ms, 3),
                    "first_proxy_204_ms": sample["since_core_start_ms"],
                    "attempts": samples}
        if sample["error_class"] == "route_evidence_missing":
            break
        time.sleep(.02)
    raise RuntimeError("no verified first proxied 204")


def cpu_seconds(text):
    days, _, clock = text.strip().rpartition('-')
    parts = [float(part) for part in clock.split(':')]
    value = sum(part * (60 ** index) for index, part in enumerate(reversed(parts)))
    return value + (int(days) * 86400 if days else 0)


def core_stats(pid):
    result = subprocess.run(["ps", "-p", str(pid), "-o", "time=", "-o", "rss="],
                            capture_output=True, text=True, timeout=2, check=True)
    clock, rss = result.stdout.split()
    return {"at": time.monotonic(), "cpu_seconds": cpu_seconds(clock), "rss_kib": int(rss)}


def resource_summary(points):
    intervals = [(b["cpu_seconds"] - a["cpu_seconds"]) / (b["at"] - a["at"]) * 100
                 for a, b in zip(points, points[1:]) if b["at"] > a["at"]]
    elapsed = points[-1]["at"] - points[0]["at"] if len(points) > 1 else 0
    return {"sampling_points": len(points), "window_ms": round(elapsed * 1000, 3),
            "cpu_avg_percent_one_core": round((points[-1]["cpu_seconds"] - points[0]["cpu_seconds"]) / elapsed * 100, 3) if elapsed > 0 else None,
            "cpu_peak_interval_percent_one_core": round(max(intervals), 3) if intervals else None,
            "rss_peak_observed_kib": max((p["rss_kib"] for p in points), default=None)}


class Resources:
    def __init__(self, pid):
        self.pid, self.points, self.errors = pid, [], []
        self.stop = threading.Event()

    def take(self):
        try:
            self.points.append(core_stats(self.pid))
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            self.errors.append(type(error).__name__)

    def __enter__(self):
        self.take()
        def sample():
            while not self.stop.wait(.1):
                self.take()
        self.worker = threading.Thread(target=sample, daemon=True)
        self.worker.start()
        return self

    def __exit__(self, *_args):
        self.stop.set()
        self.worker.join(timeout=3)
        self.take()

    def result(self):
        return {**resource_summary(self.points), "errors": sorted(set(self.errors)),
                "scope": "throughput sampling window; child CPU only; sampled RSS",
                "cpu_clock_precision": "ps TIME platform precision"}


def throughput(core, mixed, upstream_port, size_mib, load_seconds, timeout):
    samples, bytes_read = [], 0
    with Resources(core.process.pid) as resources:
        started = time.monotonic()
        while not samples or time.monotonic() - started < load_seconds:
            sample = measured_fetch(core, mixed, f"http://127.0.0.1:{upstream_port}/body?mib={size_mib}",
                                    timeout=timeout, expected_status=200, expected_bytes=size_mib * MIB)
            samples.append(sample)
            if not sample["ok"]:
                break
            bytes_read += sample["bytes_read"]
        elapsed = time.monotonic() - started
    valid = all(sample["ok"] for sample in samples)
    return {"ok": valid, "payload_mib_per_request": size_mib, "minimum_load_seconds": load_seconds,
            "bytes_verified": bytes_read, "elapsed_ms": round(elapsed * 1000, 3),
            "mib_per_second": round(bytes_read / MIB / elapsed, 3) if valid and elapsed > 0 else None,
            "transfers": samples, "resources": resources.result()}


DNS_CODE = '''import json, socket, time, sys
start = time.monotonic()
try:
    answers = socket.getaddrinfo(sys.argv[1], 443, proto=socket.IPPROTO_TCP)
    result = {"ok": True, "families": sorted(set(a[0] for a in answers)), "error_class": None}
except socket.gaierror:
    result = {"ok": False, "error_class": "dns"}
result["elapsed_ms"] = (time.monotonic() - start) * 1000
print(json.dumps(result))
'''


def dns_baseline(count, timeout):
    samples = []
    for _ in range(count):
        try:
            result = subprocess.run([sys.executable, "-c", DNS_CODE, DNS_TARGET],
                                    capture_output=True, text=True, timeout=timeout, check=True)
            samples.append(json.loads(result.stdout))
        except subprocess.TimeoutExpired:
            samples.append({"ok": False, "elapsed_ms": timeout * 1000, "error_class": "timeout"})
        except (OSError, ValueError, subprocess.SubprocessError):
            samples.append({"ok": False, "elapsed_ms": None, "error_class": "resolver_worker"})
    return {"target": DNS_TARGET, "path": "system getaddrinfo; cache and physical egress unverified",
            "worker_startup_excluded": True, **summarize(samples)}


def tls_metadata():
    paths = ssl.get_default_verify_paths()
    info = {"trust_model": "Python default context; certificate verification required",
            "default_ca_file_available": bool(paths.cafile),
            "default_ca_directory_available": bool(paths.capath)}
    try:
        info["loaded_ca_certificates_at_start"] = len(ssl.create_default_context().get_ca_certs())
    except ssl.SSLError:
        info["context_error"] = "tls_context_initialization_failed"
    return info


def hardware_metadata():
    info = {"logical_cpus": os.cpu_count(), "cpu_model": platform.processor() or None,
            "python_version": platform.python_version(), "memory_bytes": None}
    if sys.platform == 'darwin':
        for key, field in [('machdep.cpu.brand_string', 'cpu_model'), ('hw.memsize', 'memory_bytes')]:
            try:
                value = subprocess.run(['/usr/sbin/sysctl', '-n', key], capture_output=True,
                                       text=True, timeout=1, check=True).stdout.strip()
                info[field] = int(value) if field == 'memory_bytes' else value
            except (OSError, ValueError, subprocess.SubprocessError):
                pass  # Missing hardware metadata stays null rather than being invented.
    return info


def source_metadata():
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True)
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True, text=True)
    return {"git_commit": commit.stdout.strip() if commit.returncode == 0 else None,
            "worktree_dirty": bool(dirty.stdout.strip()) if dirty.returncode == 0 else None,
            "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}


def save_result(output, result):
    output.mkdir(parents=True, exist_ok=True)
    path = output / (datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ") + '-' + uuid.uuid4().hex + '.json')
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', prefix='.benchmark-',
                                         suffix='.tmp', dir=output, delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(result, stream, indent=2, ensure_ascii=False)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
        return path
    finally:
        if temporary:
            temporary.unlink(missing_ok=True)


def run(core_bin, output, samples=30, size_mib=8, rounds=2, public_samples=0,
        dns_samples=0, load_seconds=1, timeout=5, engine="mihomo"):
    if engine not in ("mihomo", "sing-box"):
        raise ValueError(f"unsupported engine: {engine}")
    result = {"schema_version": 2, "status": "failed", "mode": "explicit-proxy", "engine": engine,
              "core_outbound_intent": "DIRECT", "physical_egress": "unverified",
              "source": source_metadata(), "env": {"os": platform.platform(), "arch": platform.machine(),
              "hardware": hardware_metadata(), "client_tls": tls_metadata(),
              "recorded_at": datetime.now(timezone.utc).isoformat()},
              "params": {"latency_samples_per_round": samples, "size_mib": size_mib, "rounds": rounds,
              "public_samples_per_target_per_round": public_samples, "dns_samples": dns_samples,
              "load_seconds": load_seconds, "request_timeout_seconds": timeout}, "rounds": []}
    owned, phase = [], "preflight"
    try:
        binary = core_bin.resolve(strict=True)
        version_argv = ["-v"] if engine == "mihomo" else ["version"]
        version = subprocess.run([str(binary), *version_argv], capture_output=True, text=True, timeout=5, check=True)
        result["core"] = {"version": version.stdout.strip(), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
        if engine not in version.stdout.lower():
            raise ValueError(f"binary is not {engine}: {version.stdout.strip()[:80]}")
        with tempfile.TemporaryDirectory(prefix="crossbow-bench-") as temp, upstream() as up:
            workdir = Path(temp)
            mixed = free_port({up})
            controller = free_port({up, mixed})
            secret = uuid.uuid4().hex
            config, config_name = fixture(mixed, controller, secret, engine)
            config_path = workdir / config_name
            config_path.write_text(json.dumps(config))
            config_path.chmod(0o600)
            result["fixture"] = json.loads(json.dumps(config).replace(secret, "<ephemeral redacted>"))
            result["targets"] = {"local_204": f"http://127.0.0.1:{up}/204", "public_https": list(PROBE_TARGETS)}
            phase = "negative_control"
            negative = proxied_fetch(free_port({up, mixed, controller}), result["targets"]["local_204"], timeout=1)
            result["negative_control"] = negative
            if negative["ok"]:
                raise RuntimeError("closed proxy accepted a loopback request: invalid measurement path")
            with ExitStack() as cleanup:
                phase = "cold_start"
                first = Core(binary, workdir, config_path, controller, secret, 'first.log', engine)
                owned.append(first)
                core = cleanup.enter_context(first)
                result["cold_start"] = first_request(core, mixed, result["targets"]["local_204"])
                for index in range(rounds):
                    phase = f"round_{index + 1}"
                    local = [measured_fetch(core, mixed, result["targets"]["local_204"], timeout=timeout) for _ in range(samples)]
                    round_result = {"local_http_204": summarize(local), "public_https": {}}
                    result["rounds"].append(round_result)
                    if not all(sample["ok"] for sample in local):
                        raise RuntimeError("local latency path failed verification")
                    for target in PROBE_TARGETS:
                        entries = [measured_fetch(core, mixed, target, timeout=timeout) for _ in range(public_samples)]
                        round_result["public_https"][target] = summarize(entries)
                    round_result["throughput"] = throughput(core, mixed, up, size_mib, load_seconds, timeout)
                    if not round_result["throughput"]["ok"]:
                        raise RuntimeError("local throughput bytes or path failed verification")
                phase = "system_resolver"
                result["system_resolver"] = dns_baseline(dns_samples, timeout)
                phase = "controlled_process_restart"
                started = time.monotonic()
                core.close()
                stop_ms = (time.monotonic() - started) * 1000
                restarted = Core(binary, workdir, config_path, controller, secret, 'restart.log', engine)
                owned.append(restarted)
                restarted = cleanup.enter_context(restarted)
                first_after_restart = first_request(restarted, mixed, result["targets"]["local_204"])
                result["controlled_process_restart"] = {
                    "stop_method": "terminate", "stop_ms": round(stop_ms, 3),
                    "startup": first_after_restart,
                    "total_stop_start_proxy_204_ms": round((restarted.started - started) * 1000 + first_after_restart["first_proxy_204_ms"], 3),
                    "scope": "standalone core restart, not network outage or Crossbow application recovery"}
            result["status"] = "completed"
    except (Exception, KeyboardInterrupt) as error:
        result["failure"] = {"phase": phase, "class": type(error).__name__, "reason": str(error)[:240]}
    finally:
        result["cleanup"] = {"owned_core_pids": [c.process.pid for c in owned if c.process],
                             "all_owned_cores_reaped": all(c.process is None or c.process.poll() is not None for c in owned)}
        if not result["cleanup"]["all_owned_cores_reaped"]:
            result["status"] = "failed"
        if "failure" in result:
            result["failure"]["owned_core_log_tails"] = [getattr(c, "log_tail", "") for c in owned]
    result["probe_failures"] = {
        "public_https": sum(group["count"] - group["ok_count"] for r in result["rounds"] for group in r["public_https"].values()),
        "system_resolver": result.get("system_resolver", {}).get("count", 0) - result.get("system_resolver", {}).get("ok_count", 0)}
    path = save_result(output, result)
    return result, path


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--core', type=Path, required=True)
    parser.add_argument('--engine', choices=['mihomo', 'sing-box'], default='mihomo',
                        help='fixture core engine (config generation, log evidence)')
    parser.add_argument('--mode', choices=['explicit-proxy', 'sysproxy', 'tun'], default='explicit-proxy')
    parser.add_argument('--output', type=Path, default=REPO / 'target/network-benchmark')
    parser.add_argument('--samples', type=int, default=30)
    parser.add_argument('--throughput-mib', type=int, default=8)
    parser.add_argument('--rounds', type=int, default=2)
    parser.add_argument('--load-seconds', type=float, default=1)
    parser.add_argument('--public-samples', type=int, default=0, help='HTTPS samples per target per round (opt in)')
    parser.add_argument('--dns-samples', type=int, default=0, help='system resolver samples (opt in)')
    parser.add_argument('--timeout', type=float, default=5)
    parser.add_argument('--i-know-this-touches-my-network', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.mode == 'tun':
        parser.error('TUN measurement is not implemented; mixed-port probes cannot validate TUN routing')
    for name, low, high in [('samples', 1, 1000), ('throughput_mib', 1, 64), ('rounds', 1, 10),
                            ('public_samples', 0, 20), ('dns_samples', 0, 20)]:
        if not low <= getattr(args, name) <= high:
            parser.error(f'{name} must be between {low} and {high}')
    if not .1 <= args.load_seconds <= 10 or not .1 <= args.timeout <= 30:
        parser.error('load-seconds must be 0.1..10; timeout must be 0.1..30')
    result, path = run(args.core, args.output, args.samples, args.throughput_mib, args.rounds,
                       args.public_samples, args.dns_samples, args.load_seconds, args.timeout,
                       args.engine)
    print(json.dumps({'mode': result['mode'], 'status': result['status'], 'result': str(path), 'failure': result.get('failure'), 'probe_failures': result['probe_failures']}, ensure_ascii=False))
    return 0 if result['status'] == 'completed' else 1


if __name__ == '__main__':
    sys.exit(main())
