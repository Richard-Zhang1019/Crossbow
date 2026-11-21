"""构建桌面安装包，输出保持实时可见。"""
from pathlib import Path
from collections import deque
import os
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]


def packaging_diagnostics():
    """失败现场只读诊断；不强制卸载 runner 上无法确认归属的磁盘。"""
    for command in [['/usr/bin/hdiutil', 'info'], ['/bin/df', '-h']]:
        print(f'DMG failure diagnostics: {command!r}', flush=True)
        try:
            result = subprocess.run(command, capture_output=True, text=True,
                                    encoding='utf-8', errors='replace', timeout=10,
                                    check=False)
            print(result.stdout, end='', flush=True)
            print(result.stderr, end='', flush=True)
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f'Diagnostic unavailable: {error}', flush=True)


def main():
    """有限重试下载/DMG 打包错误，编译和最终失败仍原样返回。"""
    # Windows 重定向输出默认使用 cp1252，无法打印构建工具的勾号或中文。
    sys.stdout.reconfigure(encoding='utf-8')
    command = ['pnpm.cmd' if os.name == 'nt' else 'pnpm',
               'tauri', 'build', '--config', 'tauri.ci.conf.json', '--verbose']
    for attempt in range(1, 4):
        tail = deque(maxlen=80)
        with subprocess.Popen(command, cwd=ROOT / 'apps/desktop',
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              text=True, encoding='utf-8', errors='replace', bufsize=1) as process:
            for line in process.stdout:
                print(line, end='', flush=True)
                tail.append(line)
            status = process.wait()
        if status == 0:
            return 0
        output = ''.join(tail)
        download_failed = ('failed to bundle project' in output
                           and re.search(r'http status:\s*(?:408|429|5\d\d)\b', output))
        dmg_failed = (sys.platform == 'darwin'
                      and 'failed to bundle project' in output
                      and 'error running bundle_dmg.sh' in output)
        if dmg_failed:
            packaging_diagnostics()
        if not (download_failed or dmg_failed) or attempt == 3:
            return status
        if dmg_failed:
            # Rust and the app bundle already succeeded. Re-run only packaging,
            # retaining the same CI config; never turn a bundling failure green.
            command = [command[0], 'tauri', 'bundle', '--config',
                       'tauri.ci.conf.json', '--bundles', 'dmg', '--verbose']
            reason = 'DMG packaging'
        else:
            reason = 'Packaging download'
        print(f'{reason} failed; retrying ({attempt}/2).', flush=True)
        time.sleep(15 * attempt)


if __name__ == '__main__':
    raise SystemExit(main())
