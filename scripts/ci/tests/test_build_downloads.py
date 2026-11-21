import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
import urllib.error
import urllib.request
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import build_desktop
import download_sidecar


class BuildDownloadsTests(unittest.TestCase):
    def process(self, output, status):
        """模拟一轮打包进程，保留真实的流式输出接口。"""
        process = MagicMock()
        process.__enter__.return_value = process
        process.stdout = io.StringIO(output)
        process.wait.return_value = status
        return process

    def test_packaging_download_failure_retries(self):
        with patch.object(build_desktop.subprocess, 'Popen', side_effect=[
            self.process('failed to bundle project: http status: 504\n', 1),
            self.process('installer built\n', 0),
        ]) as spawn, patch('time.sleep'):
            self.assertEqual(build_desktop.main(), 0)
        self.assertEqual(spawn.call_count, 2)

    def test_compiler_error_is_not_retried(self):
        with patch.object(build_desktop.subprocess, 'Popen', return_value=
                          self.process('error[E0433]: unresolved type\n', 101)) as spawn, \
             patch('time.sleep') as sleep:
            self.assertEqual(build_desktop.main(), 101)
        self.assertEqual(spawn.call_count, 1)
        sleep.assert_not_called()

    def test_windows_output_can_print_unicode_build_progress(self):
        """用 Windows 默认控制台编码重现并验证 Unicode 日志输出。"""
        output = io.BytesIO()
        console = io.TextIOWrapper(output, encoding='cp1252', write_through=True)
        with patch('sys.stdout', console), \
             patch.object(build_desktop.subprocess, 'Popen', return_value=
                          self.process('✓ built\n构建成功\n', 0)):
            self.assertEqual(build_desktop.main(), 0)
        console.flush()
        self.assertEqual(output.getvalue().decode('utf-8'), '✓ built\n构建成功\n')

    def test_repeated_download_failure_stays_failed(self):
        processes = [self.process('failed to bundle project: http status: 504\n', 1)
                     for _ in range(3)]
        with patch.object(build_desktop.subprocess, 'Popen', side_effect=processes) as spawn, \
             patch('time.sleep'):
            self.assertEqual(build_desktop.main(), 1)
        self.assertEqual(spawn.call_count, 3)

    def test_macos_dmg_failure_retries_only_bundling(self):
        failure = 'failed to bundle project: error running bundle_dmg.sh\n'
        with patch.object(build_desktop.sys, 'platform', 'darwin'), \
             patch.object(build_desktop.subprocess, 'Popen', side_effect=[
                 self.process(failure, 1), self.process('DMG built\n', 0),
             ]) as spawn, patch.object(build_desktop, 'packaging_diagnostics') as diagnostics, \
             patch('time.sleep'):
            self.assertEqual(build_desktop.main(), 0)
        first = spawn.call_args_list[0].args[0]
        retry = spawn.call_args_list[1].args[0]
        self.assertEqual(first[1:3], ['tauri', 'build'])
        self.assertIn('--verbose', first)
        self.assertEqual(retry[1:], ['tauri', 'bundle', '--config',
                                    'tauri.ci.conf.json', '--bundles', 'dmg', '--verbose'])
        diagnostics.assert_called_once()

    def test_repeated_dmg_failure_returns_last_failure(self):
        failure = 'failed to bundle project: error running bundle_dmg.sh\n'
        with patch.object(build_desktop.sys, 'platform', 'darwin'), \
             patch.object(build_desktop.subprocess, 'Popen', side_effect=[
                 self.process(failure, 1), self.process(failure, 2), self.process(failure, 7),
             ]) as spawn, patch.object(build_desktop, 'packaging_diagnostics') as diagnostics, \
             patch('time.sleep') as sleep:
            self.assertEqual(build_desktop.main(), 7)
        self.assertEqual(spawn.call_count, 3)
        self.assertEqual(diagnostics.call_count, 3)
        self.assertEqual(sleep.call_count, 2)
        self.assertTrue(all(call.args[0][2] == 'bundle' for call in spawn.call_args_list[1:]))

    def test_other_platform_and_other_packaging_errors_are_not_dmg_retried(self):
        for platform, output in [
            ('win32', 'failed to bundle project: error running bundle_dmg.sh\n'),
            ('darwin', 'failed to bundle project: code signing failed\n'),
            ('darwin', 'error[E0433]: unresolved type\n'),
        ]:
            with self.subTest(platform=platform, output=output), \
                 patch.object(build_desktop.sys, 'platform', platform), \
                 patch.object(build_desktop.subprocess, 'Popen', return_value=self.process(output, 1)) as spawn, \
                 patch.object(build_desktop, 'packaging_diagnostics') as diagnostics, \
                 patch('time.sleep') as sleep:
                self.assertEqual(build_desktop.main(), 1)
                self.assertEqual(spawn.call_count, 1)
                diagnostics.assert_not_called()
                sleep.assert_not_called()

    def test_diagnostic_failure_does_not_hide_packaging_failure(self):
        with patch.object(build_desktop.subprocess, 'run', side_effect=
                          build_desktop.subprocess.TimeoutExpired('hdiutil', 10)) as run:
            build_desktop.packaging_diagnostics()
        self.assertEqual(run.call_count, 2)

    def test_api_fallback_authenticates_metadata_and_asset_without_redirecting_token(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'apps/desktop/src-tauri/src/engine/download.rs'
            source.parent.mkdir(parents=True)
            source.write_text('pub const CORE_VERSION: &str = "v1.19.32";', encoding='utf-8')
            data = gzip.compress(b'fixture-mihomo')
            digest = hashlib.sha256(data).hexdigest()
            api_requests = []

            def open_url(request, **_):
                """主镜像失败后，仅 GitHub API 接收不随重定向转发的鉴权头。"""
                url = request.full_url if isinstance(request, urllib.request.Request) else request
                if not url.startswith('https://api.github.com/'):
                    raise urllib.error.HTTPError(url, 504, 'Gateway timeout', {}, None)
                self.assertIsInstance(request, urllib.request.Request)
                self.assertEqual(request.get_header('Authorization'), 'Bearer fixture-token')
                self.assertNotIn('Authorization', request.headers)
                api_requests.append(url)
                if '/tags/' in url:
                    return io.BytesIO(json.dumps({'assets': [{'id': 42, 'name': 'fixture.gz'}]}).encode())
                return io.BytesIO(data)

            with patch.object(download_sidecar, 'ROOT', root), \
                 patch.object(download_sidecar, 'ASSETS', {'fixture': ('fixture.gz', digest)}), \
                 patch.object(sys, 'argv', ['download_sidecar.py', 'fixture']), \
                 patch.dict(os.environ, {'GITHUB_TOKEN': 'fixture-token'}, clear=True), \
                 patch.object(download_sidecar.urllib.request, 'urlopen', side_effect=open_url), \
                 patch('time.sleep'):
                download_sidecar.main()
            self.assertEqual(len(api_requests), 2)
            self.assertEqual((root / 'apps/desktop/src-tauri/binaries/mihomo-fixture').read_bytes(),
                             b'fixture-mihomo')


if __name__ == '__main__':
    unittest.main()
