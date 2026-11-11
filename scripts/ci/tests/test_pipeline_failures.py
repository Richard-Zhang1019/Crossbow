"""Exercise failure control flow without Apple credentials or launching the app."""
import contextlib
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import smoke_macos

NOTARY_SCRIPT = Path(__file__).resolve().parents[1] / 'notarize_macos.sh'


class NotaryFailures(unittest.TestCase):
    def run_notary(self, response, status=0):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            mock = root / 'xcrun'
            mock.write_text('''#!/usr/bin/env bash
printf '%s\\n' "$1 $2" >> "$CALL_LOG"
if [ "$1 $2" = 'notarytool submit' ]; then
  printf '%s' "$NOTARY_RESPONSE"
  exit "$NOTARY_STATUS"
elif [ "$1 $2" = 'notarytool log' ]; then
  echo '{"issues":[{"message":"fixture rejection"}]}'
fi
''')
            mock.chmod(0o755)
            calls = root / 'calls'
            result = subprocess.run(['bash', str(NOTARY_SCRIPT), 'fixture.dmg'], cwd=root,
                capture_output=True, text=True, env=dict(os.environ,
                    PATH=str(root) + os.pathsep + os.environ['PATH'],
                    APPLE_ID='fixture', APPLE_PASSWORD='fixture', APPLE_TEAM_ID='fixture',
                    CALL_LOG=str(calls), NOTARY_RESPONSE=response, NOTARY_STATUS=str(status)))
            return result, calls.read_text()

    def test_accepted_submission_staples_and_validates(self):
        result, calls = self.run_notary('{"id":"fixture-id","status":"Accepted"}')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('stapler staple', calls)
        self.assertIn('stapler validate', calls)

    def test_rejected_submission_collects_log_and_never_staples(self):
        result, calls = self.run_notary('{"id":"fixture-id","status":"Invalid"}')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('notarytool log', calls)
        self.assertIn('fixture rejection', result.stdout)
        self.assertNotIn('stapler', calls)

    def test_auth_failure_without_json_never_staples(self):
        result, calls = self.run_notary('', 1)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('notarytool log', calls)
        self.assertNotIn('stapler', calls)

    def test_timeout_collects_log_and_never_staples(self):
        result, calls = self.run_notary('{"id":"fixture-id","status":"In Progress"}', 1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('notarytool log', calls)
        self.assertNotIn('stapler', calls)


class StartupFailures(unittest.TestCase):
    def test_refuses_developer_machine(self):
        with patch.dict(os.environ, {'RUNNER_ENVIRONMENT': ''}):
            with self.assertRaisesRegex(RuntimeError, 'disposable'):
                smoke_macos.main()

    def run_smoke(self, exit_code, fatal=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / 'Crossbow.app'
            (app / 'Contents').mkdir(parents=True)
            (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'fixture'}))
            process = MagicMock()
            process.poll.return_value = exit_code
            process.returncode = exit_code
            def launch(*args, **kwargs):
                if fatal:
                    kwargs['stderr']  # Both streams are redirected into one log.
                    kwargs['stdout'].write('crossbow fatal: fixture startup failure\n')
                    kwargs['stdout'].flush()
                return process
            with contextlib.ExitStack() as stack:
                stack.enter_context(patch.object(sys, 'platform', 'darwin'))
                stack.enter_context(patch.object(sys, 'argv', ['smoke', str(app)]))
                stack.enter_context(patch.dict(os.environ, {'RUNNER_ENVIRONMENT': 'github-hosted'}))
                stack.enter_context(patch.object(smoke_macos.subprocess, 'Popen', side_effect=launch))
                stack.enter_context(patch.object(smoke_macos.time, 'sleep'))
                stack.enter_context(patch.object(smoke_macos.os, 'killpg'))
                stack.enter_context(patch.object(smoke_macos.Path, 'home', return_value=root))
                stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
                stack.enter_context(contextlib.redirect_stderr(io.StringIO()))
                old = Path.cwd()
                os.chdir(root)
                try:
                    smoke_macos.main()
                finally:
                    os.chdir(old)
            process.wait.assert_called_once()

    def test_immediate_failure_and_clean_exit_are_both_rejected(self):
        for code in [0, -6, 7]:
            with self.subTest(code=code), self.assertRaisesRegex(RuntimeError, 'exited during startup'):
                self.run_smoke(code)

    def test_fatal_dialog_is_not_a_successful_start(self):
        with self.assertRaisesRegex(RuntimeError, 'fatal startup error'):
            self.run_smoke(None, fatal=True)

    def test_surviving_process_is_stopped(self):
        self.run_smoke(None)


if __name__ == '__main__':
    unittest.main()

class UnwrapGuardTest(unittest.TestCase):
    """#422：unwrap 护栏——毒锁清零 + command 入口零 unwrap 的阈值不被回潮。"""

    def test_guard_passes_on_current_tree(self):
        import subprocess, sys
        from pathlib import Path
        root = Path(__file__).resolve().parents[3]
        result = subprocess.run(
            [sys.executable, str(root / "scripts/ci/check_unwraps.py")],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr or result.stdout)
