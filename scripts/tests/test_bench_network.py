"""Benchmark regressions: local sockets/owned children only; no TUN or WAN."""
import importlib.util
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'benchmark/bench_network.py'
SPEC = importlib.util.spec_from_file_location('bench_network', SCRIPT)
b = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(b)


class ReplyProxy(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.paths.append(self.path)
        status = self.server.reply_status
        self.send_response(status)
        self.send_header('Content-Length', str(self.server.declared_size))
        if status == 302:
            self.send_header('Location', self.path)
        self.end_headers()
        self.wfile.write(self.server.body)

    def do_CONNECT(self):
        self.server.paths.append('CONNECT ' + self.path)
        self.send_response(502)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def log_message(self, *_args):
        pass


class BenchmarkTests(unittest.TestCase):
    def proxy(self, status=204, body=b'', declared=None):
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), ReplyProxy)
        server.paths = []
        server.reply_status, server.body = status, body
        server.declared_size = len(body) if declared is None else declared
        worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=.01), daemon=True)
        worker.start()
        def cleanup():
            server.shutdown()
            server.server_close()
            worker.join(timeout=1)
        self.addCleanup(cleanup)
        return server, server.server_address[1]

    def test_closed_proxy_cannot_bypass_to_loopback_even_with_no_proxy(self):
        with b.upstream() as up, patch.dict(os.environ, {'NO_PROXY': '*', 'no_proxy': '*'}):
            result = b.proxied_fetch(b.free_port({up}), f'http://127.0.0.1:{up}/204', timeout=.2)
        self.assertFalse(result['ok'])
        self.assertEqual(result['error_class'], 'connect')

    def test_explicit_socket_ignores_proxy_environment_and_uses_absolute_form(self):
        proxy, port = self.proxy()
        with patch.dict(os.environ, {'NO_PROXY': '*', 'no_proxy': '*', 'HTTP_PROXY': 'http://127.0.0.1:1'}):
            result = b.proxied_fetch(port, 'http://unresolvable.invalid/204')
        self.assertTrue(result['ok'])
        self.assertEqual(proxy.paths, ['http://unresolvable.invalid/204'])
        self.assertEqual(result['proxy_peer'], ['127.0.0.1', port])
        self.assertIn('127.0.0.1:', result['source'])

    def test_https_uses_connect_even_when_no_proxy_would_match(self):
        proxy, port = self.proxy()
        with patch.dict(os.environ, {'NO_PROXY': '*', 'no_proxy': '*'}):
            result = b.proxied_fetch(port, 'https://unresolvable.invalid/204')
        self.assertFalse(result['ok'])
        self.assertEqual(proxy.paths, ['CONNECT unresolvable.invalid:443'])

    def test_redirect_and_other_success_status_are_not_204_samples(self):
        for status in [200, 302, 403]:
            with self.subTest(status=status):
                proxy, port = self.proxy(status=status)
                result = b.proxied_fetch(port, 'http://unresolvable.invalid/204')
                self.assertFalse(result['ok'])
                self.assertEqual(result['http_status'], status)
                self.assertEqual(result['error_class'], 'http_status')
                self.assertEqual(len(proxy.paths), 1)

    def test_throughput_requires_the_actual_complete_byte_count(self):
        for body, declared in [(b'abc', 3), (b'abc', 10)]:
            with self.subTest(declared=declared):
                _, port = self.proxy(status=200, body=body, declared=declared)
                result = b.proxied_fetch(port, 'http://unresolvable.invalid/body', expected_status=200, expected_bytes=10)
                self.assertFalse(result['ok'])
                self.assertIn(result['error_class'], ['body_size', 'incomplete_body'])
                self.assertEqual(result['bytes_read'], 3)

    def test_failed_samples_are_retained_but_excluded_from_percentiles(self):
        result = b.summarize([{'ok': True, 'elapsed_ms': 1},
                              {'ok': False, 'elapsed_ms': 1000, 'error_class': 'timeout'},
                              {'ok': True, 'elapsed_ms': 9}])
        self.assertEqual((result['count'], result['ok_count']), (3, 2))
        self.assertEqual((result['p50_ms'], result['p95_ms']), (1, 9))
        self.assertEqual(result['failures'], {'timeout': 1})
        self.assertEqual(b.summarize([])['p95_ms'], None)

    def test_success_without_owned_core_log_is_not_a_valid_sample(self):
        _, port = self.proxy()
        core = SimpleNamespace(started=time.monotonic(), evidence=lambda sample: None)
        result = b.measured_fetch(core, port, 'http://unresolvable.invalid/204')
        self.assertFalse(result['ok'])
        self.assertEqual(result['error_class'], 'route_evidence_missing')

    def test_core_log_evidence_matches_source_tuple_not_just_direct_text(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            core = b.Core(Path('unused'), root, root/'config', 1, 'secret', 'core.log')
            core.process = SimpleNamespace(pid=123)
            core.log_path.write_text('[TCP] 127.0.0.1:1234(curl, uid=1001) --> example.test:80 match Match using DIRECT\n')
            evidence = core.evidence({'source': '127.0.0.1:1234'})
            self.assertEqual(evidence['core_reported_destination'], 'example.test:80')
            self.assertEqual(evidence['owned_core_pid'], 123)
            with patch.object(b.time, 'monotonic', side_effect=[0, 2]):
                self.assertIsNone(core.evidence({'source': '127.0.0.1:12'}))

    def test_readiness_failure_reaps_the_owned_real_child(self):
        real_popen, children = subprocess.Popen, []
        def child(*args, **kwargs):
            process = real_popen([sys.executable, '-c', 'import time; time.sleep(60)'], **kwargs)
            children.append(process)
            return process
        with tempfile.TemporaryDirectory() as temp, patch.object(b.subprocess, 'Popen', side_effect=child), \
                patch.object(b, 'wait_core_ready', side_effect=RuntimeError('readiness failed')):
            root = Path(temp)
            with self.assertRaisesRegex(RuntimeError, 'readiness failed'):
                with b.Core(Path('unused'), root, root/'config', 1, 'secret', 'log'):
                    self.fail('readiness unexpectedly passed')
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll())

    def test_mid_measurement_exception_also_reaps_the_owned_child(self):
        real_popen, children = subprocess.Popen, []
        def child(*args, **kwargs):
            process = real_popen([sys.executable, '-c', 'import time; time.sleep(60)'], **kwargs)
            children.append(process)
            return process
        with tempfile.TemporaryDirectory() as temp, patch.object(b.subprocess, 'Popen', side_effect=child), \
                patch.object(b, 'wait_core_ready'):
            root = Path(temp)
            with self.assertRaisesRegex(ValueError, 'sample failure'):
                with b.Core(Path('unused'), root, root/'config', 1, 'secret', 'log'):
                    raise ValueError('sample failure')
        self.assertIsNotNone(children[0].poll())

    def test_cpu_uses_cumulative_time_deltas_not_lifetime_ps_percent(self):
        self.assertEqual(b.cpu_seconds('1-02:03:04.50'), 93784.5)
        value = b.resource_summary([{'at': 10, 'cpu_seconds': 100, 'rss_kib': 20},
                                    {'at': 12, 'cpu_seconds': 101, 'rss_kib': 30}])
        self.assertEqual(value['cpu_avg_percent_one_core'], 50)
        self.assertEqual(value['rss_peak_observed_kib'], 30)
        self.assertIsNone(b.resource_summary([])['cpu_avg_percent_one_core'])

    def test_resolver_timeout_is_recorded_without_losing_other_samples(self):
        with patch.object(b.subprocess, 'run', side_effect=[subprocess.TimeoutExpired('resolver', 1),
             SimpleNamespace(stdout=json.dumps({'ok': True, 'elapsed_ms': 2, 'error_class': None}))]):
            result = b.dns_baseline(2, 1)
        self.assertEqual(result['ok_count'], 1)
        self.assertEqual(result['p95_ms'], 2)
        self.assertEqual(result['failures'], {'timeout': 1})

    def test_tun_and_invalid_sizes_fail_before_any_process_is_started(self):
        for args in [ ['--mode', 'tun'], ['--mode', 'tun', '--i-know-this-touches-my-network'],
                      ['--samples', '0'], ['--throughput-mib', '-1'] ]:
            with self.subTest(args=args), patch.object(b, 'run') as run, patch('sys.stderr', io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    b.main(['--core', 'unused', *args])
                self.assertEqual(error.exception.code, 2)
                run.assert_not_called()

    def test_archives_are_unique_and_failed_writes_leave_no_partial_json(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            one = b.save_result(root, {'value': 1})
            two = b.save_result(root, {'value': 2})
            self.assertNotEqual(one, two)
            self.assertEqual(json.loads(one.read_text())['value'], 1)
            with patch.object(b.json, 'dump', side_effect=ValueError('cannot serialize')):
                with self.assertRaises(ValueError):
                    b.save_result(root, {})
            self.assertEqual(set(root.iterdir()), {one, two})

    def test_failed_run_still_writes_a_private_result(self):
        with tempfile.TemporaryDirectory() as temp:
            result, path = b.run(Path(temp)/'missing-core', Path(temp)/'results')
            self.assertEqual(result['status'], 'failed')
            self.assertTrue(result['cleanup']['all_owned_cores_reaped'])
            self.assertEqual(json.loads(path.read_text())['failure']['phase'], 'preflight')
            if os.name != 'nt':
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)


if __name__ == '__main__':
    unittest.main()
