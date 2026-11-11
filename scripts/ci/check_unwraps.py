#!/usr/bin/env python3
"""#422：桌面 crate 非测试代码 .unwrap() 护栏。

- commands/* 与 lib.rs（command 入口）：不允许 unwrap（panic 即进程死亡）
- app_state.rs：不允许 unwrap
- engine/flow.rs、tray/wiring.rs、lifecycle/exit_flow.rs：watchdog/接线
  线程，上限 4 且 lock().unwrap()（毒锁）必须为 0
- 测试模块（mod tests 之后）豁免

超限退出 1，由 pipeline-check 调用。
"""
import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parents[2] / "apps/desktop/src-tauri/src"
LIMITS = {
    "commands": 0,
    "lib.rs": 0,
    "app_state.rs": 0,
    "engine/flow.rs": 4,
    "tray/wiring.rs": 4,
    "lifecycle/exit_flow.rs": 4,
}

def non_test_text(text: str) -> str:
    """测试模块（mod tests 之后）豁免。"""
    idx = text.find("\nmod tests")
    return text if idx == -1 else text[:idx]

def non_test_unwraps(text: str) -> int:
    return len(re.findall(r"\.unwrap\(\)", non_test_text(text)))

def main() -> int:
    failures = []
    failures = []
    for rel, limit in LIMITS.items():
        if rel == "commands":
            texts = {f"commands/{f.name}": f.read_text(encoding="utf-8") for f in sorted((SRC / "commands").glob("*.rs"))}
        else:
            texts = {rel: (SRC / rel).read_text(encoding="utf-8")}
        for name, text in texts.items():
            if "lock().unwrap()" in non_test_text(text):
                failures.append(f"{name}: 存在 lock().unwrap()（毒锁必须恢复为 into_inner）")
            count = non_test_unwraps(text)
            if count > limit:
                failures.append(f"{name}: 非测试 unwrap {count} 超上限 {limit}")
    if not failures:
        pass
    if failures:
        if "lock().unwrap()" in text:
            failures.append(f"{rel}: 存在 lock().unwrap()（毒锁必须恢复为 into_inner）")
        count = non_test_unwraps(text)
        if count > limit:
            failures.append(f"{rel}: 非测试 unwrap {count} 超上限 {limit}")
    if failures:
        for f in failures:
            print(f"FAIL {f}", file=sys.stderr)
        return 1
    print("unwrap guard ok")
    return 0

if __name__ == "__main__":
    sys.exit(main())
