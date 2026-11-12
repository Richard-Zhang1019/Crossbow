"""构建桌面安装包，输出保持实时可见。"""
from pathlib import Path
from collections import deque
import os
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    """仅重试安装器下载的临时 HTTP 错误，编译错误和最终失败原样返回。"""
    # Windows 重定向输出默认使用 cp1252，无法打印构建工具的勾号或中文。
    sys.stdout.reconfigure(encoding='utf-8')
    command = ['pnpm.cmd' if os.name == 'nt' else 'pnpm',
               'tauri', 'build', '--config', 'tauri.ci.conf.json']
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
        retryable = ('failed to bundle project' in output
                     and re.search(r'http status:\s*(?:408|429|5\d\d)\b', output))
        if not retryable or attempt == 3:
            return status
        print(f'Packaging download failed; retrying ({attempt}/2).', flush=True)
        time.sleep(15 * attempt)


if __name__ == '__main__':
    raise SystemExit(main())
