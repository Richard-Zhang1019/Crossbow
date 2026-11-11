#!/usr/bin/env python3
"""Interactively update Crossbow's release version references."""

import argparse
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_checks import source_version

VERSION_RE = re.compile(r"v?(\d+\.\d+\.\d+)")
README_FILES = ("README.md", "README.zh-CN.md")


def normalize_version(value):
    match = VERSION_RE.fullmatch(value.strip())
    if not match:
        raise ValueError("请输入稳定版本号，格式为 X.Y.Z（可带 v 前缀）")
    return match.group(1)


def replace_once(text, pattern, replacement, description):
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"无法唯一定位需要更新的版本：{description}")
    return updated


def update_cargo_manifest(text, old, new):
    section = re.search(
        r"(?ms)^\[workspace\.package\]\s*\n.*?(?=^\[|\Z)", text
    )
    if section is None:
        raise ValueError("Cargo.toml 中缺少 [workspace.package] 段")
    updated_section = replace_once(
        section.group(0),
        rf'(?m)^version[ \t]*=[ \t]*"{re.escape(old)}"[ \t]*$',
        f'version = "{new}"',
        "Cargo.toml workspace version",
    )
    return text[:section.start()] + updated_section + text[section.end():]


def update_cargo_lock(text, old, new, root):
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    members = cargo["workspace"].get("members", [])
    packages = {"crossbow-core", "crossbow-desktop"}
    if "apps/service" in members:
        packages.add("crossbow-service")

    locked = tomllib.loads(text)["package"]
    versions = {
        package["name"]: package["version"]
        for package in locked
        if package["name"] in packages
    }
    if set(versions) != packages or any(version != old for version in versions.values()):
        raise ValueError("Cargo.lock 中的 Crossbow package 版本与当前应用版本不一致")

    blocks = re.split(r"(?m)(?=^\[\[package\]\]$)", text)
    updated = []
    seen = set()
    for block in blocks:
        name = re.search(r'(?m)^name[ \t]*=[ \t]*"([^"]+)"[ \t]*$', block)
        if name is None or name.group(1) not in packages:
            updated.append(block)
            continue
        package_name = name.group(1)
        if package_name in seen:
            raise ValueError(f"Cargo.lock 中 {package_name} 出现多次")
        seen.add(package_name)
        updated.append(replace_once(
            block,
            rf'(?m)^version[ \t]*=[ \t]*"{re.escape(old)}"[ \t]*$',
            f'version = "{new}"',
            f"Cargo.lock {package_name}",
        ))
    if seen != packages:
        raise ValueError("Cargo.lock 缺少一个或多个 Crossbow workspace package")
    return "".join(updated)


def update_json_version(path, old, new):
    text = path.read_text(encoding="utf-8")
    data = json.loads(text)
    if data.get("version") != old:
        raise ValueError(f"{path.name} 中的版本与当前应用版本不一致")
    return replace_once(
        text,
        rf'(?m)^([ \t]*"version"[ \t]*:[ \t]*"){re.escape(old)}(")',
        rf"\g<1>{new}\g<2>",
        str(path),
    )


def update_readme(path, new):
    text = path.read_text(encoding="utf-8")
    header_pattern = re.compile(
        r"^(\*\*(?:Current version:|当前版本：)\*\* \[)v\d+\.\d+\.\d+"
        r"(\]\(https://github\.com/Richard-Zhang1019/Crossbow/releases)"
        r"(/tag/v\d+\.\d+\.\d+)?(\)[.。]?)$",
        re.MULTILINE,
    )

    def replace_header(match):
        tag_link = f"/tag/v{new}" if match.group(3) else ""
        return f"{match.group(1)}v{new}{match.group(2)}{tag_link}{match.group(4)}"

    text, header_count = header_pattern.subn(replace_header, text)
    if header_count != 1:
        raise ValueError(f"{path.name} 中未找到唯一的当前版本行")

    heading = "## Install & use" if path.name == "README.md" else "## 安装与使用"
    section_start = text.find(heading)
    if section_start < 0:
        raise ValueError(f"{path.name} 中未找到下载说明章节")
    section_end = re.search(r"(?m)^## ", text[section_start + len(heading):])
    section_stop = (
        section_start + len(heading) + section_end.start()
        if section_end
        else len(text)
    )
    section = text[section_start:section_stop]
    asset_pattern = re.compile(
        r"(https://github\.com/Richard-Zhang1019/Crossbow/releases/download/)"
        r"v(?P<tag>\d+\.\d+\.\d+)(/Crossbow_)(?P<asset>\d+\.\d+\.\d+)"
        r"(?P<suffix>_[A-Za-z0-9._-]+)"
    )

    def replace_asset(match):
        if match.group("tag") != match.group("asset"):
            raise ValueError(f"{path.name} 中发布标签与下载文件版本不一致")
        return f"{match.group(1)}v{new}{match.group(3)}{new}{match.group('suffix')}"

    section, asset_count = asset_pattern.subn(replace_asset, section)
    if asset_count == 0:
        raise ValueError(f"{path.name} 下载说明中未找到版本化发布资产链接")
    return text[:section_start] + section + text[section_stop:]


def prepare_updates(root, new_version):
    new = normalize_version(new_version)
    old = source_version(root)
    if new == old:
        raise ValueError("新版本号与当前版本相同")

    paths = {
        "Cargo.toml": root / "Cargo.toml",
        "Cargo.lock": root / "Cargo.lock",
        "apps/desktop/package.json": root / "apps/desktop/package.json",
        "apps/desktop/src-tauri/tauri.conf.json":
            root / "apps/desktop/src-tauri/tauri.conf.json",
    }
    updates = {
        paths["Cargo.toml"]: update_cargo_manifest(
            paths["Cargo.toml"].read_text(encoding="utf-8"), old, new
        ),
        paths["Cargo.lock"]: update_cargo_lock(
            paths["Cargo.lock"].read_text(encoding="utf-8"), old, new, root
        ),
        paths["apps/desktop/package.json"]: update_json_version(
            paths["apps/desktop/package.json"], old, new
        ),
        paths["apps/desktop/src-tauri/tauri.conf.json"]: update_json_version(
            paths["apps/desktop/src-tauri/tauri.conf.json"], old, new
        ),
    }
    for name in README_FILES:
        path = root / name
        updates[path] = update_readme(path, new)
    return old, new, updates


def write_updates(updates):
    staged = []
    try:
        for path, content in updates.items():
            with tempfile.NamedTemporaryFile(
                mode="w", encoding="utf-8", newline="", dir=path.parent,
                prefix=f".{path.name}.", suffix=".tmp", delete=False,
            ) as stream:
                stream.write(content)
                staged.append((Path(stream.name), path))
        for temporary, destination in staged:
            os.replace(temporary, destination)
    finally:
        for temporary, _ in staged:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run", action="store_true",
        help="show the files that would change without writing them",
    )
    args = parser.parse_args()

    current = source_version(ROOT)
    print(f"当前应用版本：{current}")
    new = normalize_version(input("请输入新版本号（X.Y.Z，可带 v 前缀）："))
    old, new, updates = prepare_updates(ROOT, new)
    print(f"\n将版本从 {old} 更新为 {new}，涉及文件：")
    for path in updates:
        print(f"  {path.relative_to(ROOT)}")
    if args.dry_run:
        print("\nDry run：未写入文件。")
        return 0
    if input("\n确认更新？[y/N]：").strip().lower() not in {"y", "yes"}:
        print("已取消，未写入文件。")
        return 0

    write_updates(updates)
    if source_version(ROOT) != new:
        raise ValueError("写入后版本校验失败")
    print(f"版本已更新为 {new}。脚本未提交、创建 tag 或触发发布。")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"版本更新失败：{error}", file=sys.stderr)
        raise SystemExit(1)
