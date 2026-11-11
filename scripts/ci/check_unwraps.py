#!/usr/bin/env python3
"""Enforce the #422 unwrap budgets in desktop startup and command code."""

import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parents[2] / "apps/desktop/src-tauri/src"
LIMITS = {
    "lib.rs": 0,
    "app_state.rs": 0,
    "engine/flow.rs": 4,
    "tray/wiring.rs": 4,
    "lifecycle/exit_flow.rs": 4,
}
UNWRAP = re.compile(r"\.\s*unwrap\s*\(\s*\)")
LOCK_UNWRAP = re.compile(r"\.\s*lock\s*\(\s*\)\s*" + UNWRAP.pattern)
TEST_MODULE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*mod\s+\w+\s*\{")
BRACE = re.compile(r"[{}]")
BLOCK_COMMENT = re.compile(r"/\*|\*/")
NON_CODE = re.compile(
    r'//[^\n]*|/\*|\br(?P<hashes>#{0,255})".*?"(?P=hashes)'
    r'|"(?:\\.|[^"\\])*"'
    r"|'(?:\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|.)|[^'\\\n])'",
    re.DOTALL,
)


def code_only(source: str) -> str:
    """Mask comments and literals so they cannot create matches or fake braces."""
    parts = []
    offset = 0
    while match := NON_CODE.search(source, offset):
        parts.append(source[offset:match.start()])
        offset = match.end()
        if match.group() == "/*":
            depth = 1
            for marker in BLOCK_COMMENT.finditer(source, offset):
                depth += 1 if marker.group() == "/*" else -1
                if depth == 0:
                    offset = marker.end()
                    break
            else:
                offset = len(source)
        parts.append(" ")
    parts.append(source[offset:])
    return "".join(parts)


def non_test_code(source: str) -> str:
    code = code_only(source)
    parts = []
    offset = 0
    while match := TEST_MODULE.search(code, offset):
        depth = 1
        for brace in BRACE.finditer(code, match.end()):
            depth += 1 if brace.group() == "{" else -1
            if depth == 0:
                parts.append(code[offset:match.start()])
                parts.append(" ")
                offset = brace.end()
                break
        else:
            # Leave malformed modules visible to the scanner and compiler.
            break
    parts.append(code[offset:])
    return "".join(parts)


def violations(source: str, limit: int) -> list[str]:
    code = non_test_code(source)
    findings = []
    if LOCK_UNWRAP.search(code):
        findings.append("存在 lock().unwrap()（毒锁必须显式处理）")
    count = len(UNWRAP.findall(code))
    if count > limit:
        findings.append(f"非测试 unwrap {count} 超上限 {limit}")
    return findings


def main() -> int:
    limits = LIMITS | {
        str(path.relative_to(SRC)): 0 for path in (SRC / "commands").rglob("*.rs")
    }
    failures = [
        f"{path}: {finding}"
        for path, limit in sorted(limits.items())
        for finding in violations((SRC / path).read_text(encoding="utf-8"), limit)
    ]
    if failures:
        for failure in failures:
            print(f"FAIL {failure}", file=sys.stderr)
        return 1
    print("unwrap guard ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
