"""Fetch a pinned sidecar; mirrors must supply exactly the same verified bytes."""
import json
import os
import gzip
import hashlib
import io
import re
from pathlib import Path
import sys
import urllib.request
import zipfile

VERSION = 'v1.19.32'
# GitHub release asset digests, reviewed against MetaCubeX/mihomo v1.19.32.
ASSETS = {
    'x86_64-unknown-linux-gnu': ('mihomo-linux-amd64-v1.19.32.gz',
        '8451100836c9eda194331c2babfad490b2faf30cebc1a04b0e76fd8ac2d35d10'),
    'aarch64-apple-darwin': ('mihomo-darwin-arm64-v1.19.32.gz',
        '3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652'),
    'x86_64-pc-windows-msvc': ('mihomo-windows-amd64-v1.19.32.zip',
        '1ac84e795b5b915446e20139677fb6cc013a0778e876e8d4c96f994750b4c4a3'),
}
# wintun 0.14.1（wintun.net 官方构建，#81）：本机下载实测 SHA-256；
# Windows TUN 依赖 wintun.dll 与内核同目录（tauri.windows.conf.json 随包分发）。
WINTUN_ZIP_SHA256 = '07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51'
ROOT = Path(__file__).resolve().parents[2]


def verify(data, expected):
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError('Sidecar SHA-256 mismatch')


def github_request(url, accept='application/vnd.github+json'):
    """API 查询和资产下载共用鉴权，令牌不随 CDN 重定向转发。"""
    request = urllib.request.Request(url, headers={'Accept': accept})
    token = os.environ.get('GITHUB_TOKEN') or os.environ.get('GH_TOKEN')
    if token:
        request.add_unredirected_header('Authorization', f'Bearer {token}')
    return request


def main():
    runtime_source = (ROOT / 'apps/desktop/src-tauri/src/core_download.rs').read_text(encoding='utf-8')
    runtime_version = re.search(r'CORE_VERSION: &str = "([^"]+)"', runtime_source)
    if runtime_version is None or runtime_version.group(1) != VERSION:
        raise ValueError('CI sidecar version must match core_download.rs CORE_VERSION')
    target = sys.argv[1]
    asset, digest = ASSETS[target]
    url = f'https://github.com/MetaCubeX/mihomo/releases/download/{VERSION}/{asset}'
    # GitHub release CDN 偶发持续 504（2026-11-08 连续多小时，gh-proxy 同源
    # 继承故障）：两轮重试（直连 + gh-proxy）。仍失败才走末路镜像
    # api.github.com 资产下载（不同基础设施域，CDN 故障时仍可用）。
    data = None
    for attempt in range(2):
        for candidate in [url, 'https://gh-proxy.com/' + url]:
            try:
                with urllib.request.urlopen(candidate, timeout=60) as response:
                    candidate_data = response.read()
                verify(candidate_data, digest)
                data = candidate_data
                break
            except Exception as error:
                print(f'Sidecar download/verification failed (attempt {attempt + 1}): {error}', file=sys.stderr)
        if data is not None:
            break
        if attempt == 0:
            import time
            time.sleep(30)
    if data is None:
        # 末路镜像：api.github.com 资产下载。查询与下载分两步且都在
        # try 内——任一失败仅打印，绝不影响前面的候选结果。
        try:
            release = json.load(urllib.request.urlopen(github_request(
                f'https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/{VERSION}'),
                timeout=30))
            asset_id = next((a['id'] for a in release.get('assets', [])
                             if a['name'] == asset), None)
            if not asset_id:
                raise ValueError(f'asset {asset} not found in release {VERSION}')
            req = github_request(
                f'https://api.github.com/repos/MetaCubeX/mihomo/releases/assets/{asset_id}',
                'application/octet-stream')
            with urllib.request.urlopen(req, timeout=180) as response:
                candidate_data = response.read()
            verify(candidate_data, digest)
            data = candidate_data
            print('Sidecar downloaded via api.github.com mirror.')
        except Exception as error:
            print(f'api.github.com mirror failed: {error}', file=sys.stderr)
    if data is None:
        raise RuntimeError('No mirror returned the expected sidecar')
    if asset.endswith('.gz'):
        binary = gzip.decompress(data)
    else:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            executables = [name for name in archive.namelist() if name.endswith('.exe')]
            if len(executables) != 1:
                raise ValueError('Expected exactly one sidecar executable')
            binary = archive.read(executables[0])
    destination = ROOT / 'apps/desktop/src-tauri/binaries' / ('mihomo-' + target + ('.exe' if asset.endswith('.zip') else ''))
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(binary)
    destination.chmod(0o755)
    print(f'Verified sidecar: {destination.name}')
    if target == 'x86_64-pc-windows-msvc':
        fetch_wintun(destination.parent)


def fetch_wintun(binaries: Path) -> None:
    """wintun.dll（#81）：随包分发，安装后与 mihomo 内核同目录。"""
    url = 'https://www.wintun.net/builds/wintun-0.14.1.zip'
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read()
    verify(data, WINTUN_ZIP_SHA256)
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        dll = archive.read('wintun/bin/amd64/wintun.dll')
    destination = binaries / 'wintun.dll'
    destination.write_bytes(dll)
    print(f'Verified sidecar: {destination.name}')


if __name__ == '__main__':
    main()
