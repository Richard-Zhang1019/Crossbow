import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import set_release_version as updater


class SetReleaseVersionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "apps/desktop/src-tauri").mkdir(parents=True)
        (self.root / "apps/desktop/package.json").write_text(
            '{\n  "version": "1.2.3",\n  "dependencies": {}\n}\n',
            encoding="utf-8",
        )
        (self.root / "apps/desktop/src-tauri/tauri.conf.json").write_text(
            '{\n  "version": "1.2.3"\n}\n', encoding="utf-8"
        )
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/crossbow-core", '
            '"apps/desktop/src-tauri", "apps/service"]\n\n'
            '[workspace.package]\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
        (self.root / "Cargo.lock").write_text(
            '[[package]]\nname = "crossbow-core"\nversion = "1.2.3"\n\n'
            '[[package]]\nname = "crossbow-desktop"\nversion = "1.2.3"\n\n'
            '[[package]]\nname = "crossbow-service"\nversion = "1.2.3"\n\n'
            '[[package]]\nname = "unrelated"\nversion = "1.2.3"\n',
            encoding="utf-8",
        )
        (self.root / "package.json").write_text(
            '{"name":"crossbow","version":"0.1.0"}\n', encoding="utf-8"
        )
        (self.root / "README.md").write_text(
            "**Current version:** [v1.2.3]"
            "(https://github.com/Richard-Zhang1019/Crossbow/releases/tag/v1.2.3).\n"
            "\n## Install & use\n"
            "| macOS | [DMG](https://github.com/Richard-Zhang1019/Crossbow/"
            "releases/download/v1.2.3/Crossbow_1.2.3_aarch64.dmg) |\n"
            "\nHistorical release v0.9.0.\n",
            encoding="utf-8",
        )
        (self.root / "README.zh-CN.md").write_text(
            "**当前版本：** [v1.2.3]"
            "(https://github.com/Richard-Zhang1019/Crossbow/releases)。\n"
            "\n## 安装与使用\n"
            "| macOS | [DMG](https://github.com/Richard-Zhang1019/Crossbow/"
            "releases/download/v1.2.3/Crossbow_1.2.3_aarch64.dmg) |\n"
            "\n历史版本 v0.9.0。\n",
            encoding="utf-8",
        )

    def test_prepares_all_app_version_files_without_touching_history(self):
        old, new, updates = updater.prepare_updates(self.root, "v1.3.0")

        self.assertEqual((old, new), ("1.2.3", "1.3.0"))
        self.assertEqual(len(updates), 6)
        self.assertIn('version = "1.3.0"', updates[self.root / "Cargo.toml"])
        lock = tomllib.loads(updates[self.root / "Cargo.lock"])
        self.assertEqual(
            {
                package["name"]: package["version"]
                for package in lock["package"]
                if package["name"].startswith("crossbow-")
            },
            {
                "crossbow-core": "1.3.0",
                "crossbow-desktop": "1.3.0",
                "crossbow-service": "1.3.0",
            },
        )
        self.assertIn('name = "unrelated"\nversion = "1.2.3"', updates[self.root / "Cargo.lock"])
        self.assertIn("Crossbow_1.3.0_aarch64.dmg", updates[self.root / "README.md"])
        self.assertIn("releases/tag/v1.3.0", updates[self.root / "README.md"])
        self.assertIn("Historical release v0.9.0", updates[self.root / "README.md"])
        self.assertIn("历史版本 v0.9.0", updates[self.root / "README.zh-CN.md"])

    def test_writes_prepared_updates_and_keeps_private_root_package_version(self):
        _, _, updates = updater.prepare_updates(self.root, "1.3.0")

        updater.write_updates(updates)

        self.assertEqual(updater.source_version(self.root), "1.3.0")
        self.assertEqual(
            json.loads((self.root / "package.json").read_text())["version"],
            "0.1.0",
        )

    def test_rejects_invalid_or_unchanged_version_before_writing(self):
        before = {
            path: path.read_text(encoding="utf-8")
            for path in self.root.rglob("*")
            if path.is_file()
        }
        for invalid in ("1.3", "v1.3.0-beta", "1.2.3"):
            with self.subTest(version=invalid), self.assertRaises(ValueError):
                updater.prepare_updates(self.root, invalid)
        after = {
            path: path.read_text(encoding="utf-8")
            for path in self.root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(before, after)

    def test_rejects_mismatched_download_tag_without_partial_updates(self):
        readme = self.root / "README.md"
        readme.write_text(
            readme.read_text(encoding="utf-8").replace(
                "releases/download/v1.2.3/Crossbow_1.2.3",
                "releases/download/v1.1.0/Crossbow_1.2.3",
            ),
            encoding="utf-8",
        )
        before = {
            path: path.read_text(encoding="utf-8")
            for path in self.root.rglob("*")
            if path.is_file()
        }

        with self.assertRaisesRegex(ValueError, "发布标签与下载文件版本不一致"):
            updater.prepare_updates(self.root, "1.3.0")

        after = {
            path: path.read_text(encoding="utf-8")
            for path in self.root.rglob("*")
            if path.is_file()
        }
        self.assertEqual(before, after)


if __name__ == "__main__":
    unittest.main()
