"""Regression checks for public-probe failures; no host networking or sudo."""

import importlib.util
import io
from contextlib import redirect_stdout
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "linux" / "e2e_vm.py"
SPEC = importlib.util.spec_from_file_location("crossbow_e2e_vm_tests", SCRIPT)
e2e = importlib.util.module_from_spec(SPEC)
# The script imports its sibling protocol helpers; keep discovery's path intact.
with patch.object(sys, "path", sys.path.copy()):
    SPEC.loader.exec_module(e2e)


class HttpsProbeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.log = Path(self.temp.name) / "core.log"
        self.source = "198.18.0.1:41234"
        self.destination = "203.0.113.2"
        self.log.write_text(
            f'[TCP] {self.source}(curl, uid=1001) --> {self.destination}:443 '
            'match Match using DIRECT\n')
        e2e.CHECKS.clear()
        self.addCleanup(e2e.CHECKS.clear)
        self.stdout = io.StringIO()

    def response(self, code):
        return code, self.source, self.destination, '443'

    def immediate_wait(self, predicate, timeout):
        if not predicate():
            raise AssertionError('TUN DIRECT connection not found')

    def verify(self):
        with redirect_stdout(self.stdout), patch.object(e2e, 'wait', self.immediate_wait):
            e2e.verify_https_probe(self.log)

    def test_upstream_403_uses_independent_successful_204(self):
        with patch.object(e2e, 'fetch', side_effect=[self.response(403), self.response(204)]) as fetch:
            self.verify()
        self.assertEqual(fetch.call_count, 2)
        self.assertNotEqual(fetch.call_args_list[0], fetch.call_args_list[1])
        self.assertIn('received 403', self.stdout.getvalue())
        self.assertEqual(len(e2e.CHECKS), 1)
        self.assertIn('DIRECT', e2e.CHECKS[0])

    def test_transport_failure_can_be_reviewed_but_is_not_itself_success(self):
        with patch.object(e2e, 'fetch', side_effect=[AssertionError('TLS certificate failed'), self.response(204)]):
            self.verify()
        self.assertIn('TLS certificate failed', self.stdout.getvalue())
        self.assertEqual(len(e2e.CHECKS), 1)

    def test_all_http_or_transport_failures_still_fail_acceptance(self):
        for failures in (
            [self.response(403), self.response(429)],
            [self.response(200), self.response(302)],
            [AssertionError('TLS failed'), AssertionError('connection timed out')],
        ):
            with self.subTest(failures=failures), patch.object(e2e, 'fetch', side_effect=failures):
                with self.assertRaisesRegex(AssertionError, 'no HTTPS 204 endpoint succeeded'):
                    self.verify()
                self.assertEqual(e2e.CHECKS, [])

    def test_success_does_not_request_unnecessary_fallback(self):
        with patch.object(e2e, 'fetch', return_value=self.response(204)) as fetch:
            self.verify()
        self.assertEqual(fetch.call_count, 1)
        self.assertEqual(len(e2e.CHECKS), 1)

    def test_success_without_same_connection_direct_log_fails_without_fallback(self):
        for text in (
            '',
            '[TCP] 198.18.0.1:40000 --> 203.0.113.2:443 using DIRECT\n',
            f'[TCP] {self.source} --> 203.0.113.2:443 using PROXY\n',
            f'[TCP] {self.source} --> 203.0.113.3:443 using DIRECT\n',
        ):
            with self.subTest(text=text), patch.object(e2e, 'fetch', return_value=self.response(204)) as fetch:
                self.log.write_text(text)
                with self.assertRaisesRegex(AssertionError, 'TUN DIRECT connection not found'):
                    self.verify()
                self.assertEqual(fetch.call_count, 1)
                self.assertEqual(e2e.CHECKS, [])

    def test_data_plane_still_requires_http_and_route_assertions(self):
        original_route = ('eth0', '192.0.2.1', 'main', '192.0.2.2')
        self.log.write_text(
            f'[TCP] {self.source} --> 1.1.1.1:80 using DIRECT\n'
            f'[TCP] {self.source} --> {self.destination}:443 using DIRECT\n')
        with patch.object(e2e, 'fetch', side_effect=[
            (301, self.source, '1.1.1.1', '80'), self.response(204)
        ]), patch.object(e2e, 'private_route', return_value=original_route), \
                patch.object(e2e, 'run', return_value='8.8.8.8 dev Crossbow table 20220'), \
                patch.object(e2e, 'wait', self.immediate_wait), redirect_stdout(self.stdout):
            e2e.verify_data_plane(self.log, original_route)
        self.assertEqual(len(e2e.CHECKS), 3)


if __name__ == '__main__':
    unittest.main()
