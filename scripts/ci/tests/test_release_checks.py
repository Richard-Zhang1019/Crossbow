import hashlib
import io
import json
import os
from pathlib import Path
import plistlib
import sys
import tarfile
import tempfile
import unittest
import zipfile
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import download_sidecar
import release_checks as checks


class ReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.app = self.root / 'Crossbow.app'
        self.macos = self.app / 'Contents/MacOS'
        self.macos.mkdir(parents=True)
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleShortVersionString': '1.2.3', 'CFBundleExecutable': 'crossbow-desktop'}))
        for name in ['crossbow-desktop', 'mihomo']:
            path = self.macos / name
            path.write_bytes(b'binary')
            path.chmod(0o755)
        (self.macos / 'alias').symlink_to('mihomo')
        self.archive = self.root / 'Crossbow.app.tar.gz'
        self.repack()
        self.signature = self.root / 'Crossbow.app.tar.gz.sig'
        self.signature.write_text('signed-archive\n')
        self.manifest = self.root / 'latest.json'
        self.data = {'version': '1.2.3', 'platforms': {'darwin-aarch64': {
            'signature': 'signed-archive',
            'url': 'https://github.com/example/app/releases/download/v1.2.3/Crossbow.app.tar.gz'}}}
        self.write_manifest()

    def repack(self):
        with tarfile.open(self.archive, 'w:gz', dereference=False) as archive:
            archive.add(self.app, arcname=self.app.name)

    def write_manifest(self):
        self.manifest.write_text(json.dumps(self.data))

    def validate(self):
        checks.check_updater(self.archive, self.signature, self.manifest,
                             self.app, '1.2.3', 'v1.2.3', 'example/app')

    def test_matching_bundle_archive_and_manifest(self):
        self.validate()

    def test_stale_updater_is_rejected(self):
        (self.macos / 'crossbow-desktop').write_bytes(b'new-signature-or-build')
        with self.assertRaisesRegex(ValueError, 'content differs'):
            self.validate()

    def test_manifest_version_signature_and_url_mismatch(self):
        for key, value in [('version', '0.8.0'), ('signature', 'wrong'), ('url', 'https://example.com/wrong')]:
            with self.subTest(field=key):
                target = self.data if key == 'version' else self.data['platforms']['darwin-aarch64']
                original = target[key]
                target[key] = value
                self.write_manifest()
                with self.assertRaises(ValueError):
                    self.validate()
                target[key] = original

    def test_missing_sidecar_is_rejected(self):
        (self.macos / 'mihomo').unlink()
        with self.assertRaisesRegex(ValueError, 'mihomo'):
            checks.check_app(self.app, '1.2.3')

    def test_lost_executable_mode_is_rejected(self):
        (self.macos / 'crossbow-desktop').chmod(0o644)
        with self.assertRaisesRegex(ValueError, 'not executable'):
            self.validate()

    def test_archive_mode_change_is_rejected(self):
        (self.macos / 'mihomo').chmod(0o744)
        self.repack()
        (self.macos / 'mihomo').chmod(0o755)
        with self.assertRaisesRegex(ValueError, 'mode mismatch'):
            self.validate()

    def test_changed_symlink_is_rejected(self):
        (self.macos / 'alias').unlink()
        (self.macos / 'alias').symlink_to('crossbow-desktop')
        with self.assertRaisesRegex(ValueError, 'Symlink mismatch'):
            self.validate()

    def test_extra_or_unsafe_archive_entry_is_rejected(self):
        for name in ['Crossbow.app/Contents/extra', '../escape', 'Crossbow.app/../escape']:
            self.repack()
            with tarfile.open(self.archive, 'r:gz') as original:
                entries = [(m, original.extractfile(m).read() if m.isfile() else None) for m in original.getmembers()]
            with tarfile.open(self.archive, 'w:gz') as archive:
                for member, data in entries:
                    archive.addfile(member, io.BytesIO(data) if data is not None else None)
                member = tarfile.TarInfo(name)
                member.size = 1
                archive.addfile(member, io.BytesIO(b'x'))
            with self.subTest(path=name), self.assertRaises(ValueError):
                self.validate()

    def test_source_versions_and_tag(self):
        config = self.root / 'apps/desktop/src-tauri'
        config.mkdir(parents=True)
        (config / 'tauri.conf.json').write_text('{"version":"1.2.3"}')
        package = self.root / 'apps/desktop/package.json'
        package.write_text('{"version":"1.2.3"}')
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion="1.2.3"\n')
        lock = self.root / 'Cargo.lock'
        lock.write_text('[[package]]\nname="crossbow-core"\nversion="1.2.3"\n[[package]]\nname="crossbow-desktop"\nversion="1.2.3"\n')
        self.assertEqual(checks.source_version(self.root, 'v1.2.3'), '1.2.3')
        with self.assertRaisesRegex(ValueError, 'Tag'):
            checks.source_version(self.root, 'v1.2.4')
        package.write_text('{"version":"0.8.0"}')
        with self.assertRaisesRegex(ValueError, 'versions must agree'):
            checks.source_version(self.root)
        package.write_text('{"version":"1.2.3"}')
        lock.write_text('package=[]')
        with self.assertRaisesRegex(ValueError, 'Cargo.lock'):
            checks.source_version(self.root)

    def test_service_version_is_checked_when_in_workspace(self):
        self.test_source_versions_and_tag()
        cargo = self.root / 'Cargo.toml'
        cargo.write_text('[workspace]\nmembers=["apps/service"]\n'
                         '[workspace.package]\nversion="1.2.3"\n')
        lock = self.root / 'Cargo.lock'
        packages = ('[[package]]\nname="crossbow-core"\nversion="1.2.3"\n'
                    '[[package]]\nname="crossbow-desktop"\nversion="1.2.3"\n')
        for service in ['', '[[package]]\nname="crossbow-service"\nversion="1.2.2"\n']:
            with self.subTest(service=service):
                lock.write_text(packages + service)
                with self.assertRaisesRegex(ValueError, 'Cargo.lock'):
                    checks.source_version(self.root)
        lock.write_text(packages + '[[package]]\nname="crossbow-service"\nversion="1.2.3"\n')
        self.assertEqual(checks.source_version(self.root), '1.2.3')

    def test_windows_default_encoding_does_not_break_sidecar_download(self):
        source = self.root / 'apps/desktop/src-tauri/src/engine/download.rs'
        source.parent.mkdir(parents=True)
        source.write_text('# 源码包含中文注释\npub const CORE_VERSION: &str = "v1.19.32";', encoding='utf-8')
        # Prove this fixture reproduces the hosted runner's cp1252 failure.
        with self.assertRaises(UnicodeDecodeError):
            source.read_text(encoding='cp1252')
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, 'w') as archive:
            archive.writestr('mihomo.exe', b'fixture-executable')
        data = buffer.getvalue()
        digest = hashlib.sha256(data).hexdigest()
        target = 'x86_64-pc-windows-msvc'
        read_text = Path.read_text

        def windows_read(path, *args, **kwargs):
            kwargs.setdefault('encoding', 'cp1252')
            return read_text(path, *args, **kwargs)

        with patch.object(download_sidecar, 'ROOT', self.root), \
             patch.object(download_sidecar, 'ASSETS', {target: ('fixture.zip', digest)}), \
             patch.object(sys, 'argv', ['download_sidecar.py', target]), \
             patch.object(download_sidecar.urllib.request, 'urlopen', return_value=io.BytesIO(data)), \
             patch.object(download_sidecar, 'fetch_wintun', lambda *_: None), \
             patch.object(Path, 'read_text', windows_read):
            download_sidecar.main()
        output = self.root / 'apps/desktop/src-tauri/binaries/mihomo-x86_64-pc-windows-msvc.exe'
        self.assertEqual(output.read_bytes(), b'fixture-executable')

    def test_corrupted_sidecar_is_rejected(self):
        digest = hashlib.sha256(b'expected').hexdigest()
        download_sidecar.verify(b'expected', digest)
        with self.assertRaisesRegex(ValueError, 'SHA-256'):
            download_sidecar.verify(b'mirror error page', digest)


if __name__ == '__main__':
    unittest.main()
