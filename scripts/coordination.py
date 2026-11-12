#!/usr/bin/env python3
"""Local worktree coordination. Only writes under Git's common directory."""

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import time
import uuid


class CoordinationError(Exception):
    pass


def git(*args):
    result = subprocess.run(["git", *args], text=True, encoding="utf-8", capture_output=True)
    if result.returncode:
        raise CoordinationError(result.stderr.strip())
    return result.stdout.strip()


def context():
    root = Path(git("rev-parse", "--show-toplevel")).resolve()
    common = Path(git("rev-parse", "--git-common-dir")).resolve()
    return root, git("rev-parse", "--abbrev-ref", "HEAD"), common / "agent-coordination"


@contextmanager
def locked(directory):
    directory.mkdir(parents=True, exist_ok=True)
    lock = directory / ".lock"
    deadline = time.monotonic() + 5
    while True:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            if time.monotonic() >= deadline:
                raise CoordinationError(
                    f"协作锁忙：{lock}。若会话异常退出，核实 owner.json 中进程已结束后再人工清理；不会自动抢锁。"
                )
            time.sleep(0.05)
    try:
        (lock / "owner.json").write_text(
            json.dumps({"pid": os.getpid(), "at": timestamp()}), encoding="utf-8"
        )
        yield
    finally:
        (lock / "owner.json").unlink(missing_ok=True)
        lock.rmdir()


def timestamp():
    return datetime.now(timezone.utc).isoformat()


def read_records(directory):
    records = []
    active = {}
    for path in sorted(directory.glob("*.jsonl")):
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            try:
                record = json.loads(line)
                session = record["session"]
                if session != path.stem or record["version"] != 1:
                    raise ValueError("会话或格式不匹配")
                if record["event"] == "claim":
                    if session in active:
                        raise ValueError("上次认领尚未完成")
                    for key in ("claim_id", "worktree", "branch", "files", "issue", "summary", "at"):
                        if key not in record:
                            raise ValueError(f"缺少 {key}")
                    active[session] = record
                elif record["event"] == "finish":
                    if active.get(session, {}).get("claim_id") != record["claim_id"]:
                        raise ValueError("完成记录没有对应认领")
                    del active[session]
                else:
                    raise ValueError("未知事件")
                records.append(record)
            except (ValueError, KeyError, TypeError) as error:
                raise CoordinationError(f"协作流水损坏：{path}:{number}（{error}）；停止操作，请原会话检查。")
    return records, active


def append_record(directory, record):
    path = directory / f"{record['session']}.jsonl"
    # Global directory lock covers conflict checks AND the append. No snapshot
    # file needs replacing, and each session's journal remains append-only.
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    with os.fdopen(descriptor, "a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, ensure_ascii=False) + "\n")
        stream.flush()
        os.fsync(stream.fileno())


def normalize_files(root, files):
    result = set()
    for value in files:
        if Path(value).is_absolute() or any(char in value for char in "*?[]"):
            raise CoordinationError("--file 必须是仓库根目录相对路径，不能使用通配符。")
        try:
            path = (root / value).resolve().relative_to(root).as_posix()
        except ValueError:
            raise CoordinationError(f"路径不属于当前 worktree：{value}")
        if path == ".git" or path.startswith(".git/"):
            raise CoordinationError("不能认领 Git 元数据路径。")
        result.add(path)
    return sorted(result)


def overlaps(left, right):
    a, b = PurePosixPath(left), PurePosixPath(right)
    return a == b or a in b.parents or b in a.parents


def run(args):
    root, branch, directory = context()
    if args.command == "path":
        print(directory)
        return
    with locked(directory):
        records, active = read_records(directory)
        if args.command == "status":
            rows = list(active.values())
            if args.json:
                print(json.dumps(rows, ensure_ascii=False, indent=2))
            else:
                print(f"共享目录：{directory}")
                if not rows:
                    print("无当前认领。旧归档不代表当前状态；仍在进行的任务需各会话自行登记。")
                for row in rows:
                    print(f"{row['session']} | #{row['issue']} | {row['branch']} | {row['worktree']}")
                    print(f"  {row['summary']}\n  文件：{', '.join(row['files'])}")
            return
        if args.command == "history":
            for row in records:
                if args.session is None or row["session"] == args.session:
                    print(json.dumps(row, ensure_ascii=False))
            return
        if branch in ("HEAD", "main"):
            raise CoordinationError("请先进入自己的工作分支；不能在 main 或游离 HEAD 上认领/完成。")
        if not re.fullmatch(r"[a-z][a-z0-9_-]{0,63}", args.session):
            raise CoordinationError("会话名只能使用小写字母、数字、连字符和下划线，且以字母开头。")
        if args.command == "claim":
            if args.session in active:
                raise CoordinationError(f"{args.session} 已有未完成认领，请先完成或中止原任务。")
            files = normalize_files(root, args.file)
            for other in active.values():
                if other["worktree"] == str(root) or other["branch"] == branch:
                    raise CoordinationError(f"worktree/分支已由 {other['session']} 认领。")
                if any(overlaps(a, b) for a in files for b in other["files"]):
                    raise CoordinationError(f"文件范围与 {other['session']} 的 #{other['issue']} 重叠，请先协调。")
            record = {
                "version": 1, "event": "claim", "claim_id": uuid.uuid4().hex,
                "session": args.session, "at": timestamp(), "issue": args.issue,
                "branch": branch, "worktree": str(root), "files": files, "summary": args.summary,
            }
        else:
            previous = active.get(args.session)
            if previous is None:
                raise CoordinationError("该会话没有当前认领。")
            if previous["worktree"] != str(root) or previous["branch"] != branch:
                raise CoordinationError("只能在原认领的 worktree/分支完成任务，不能释放他人认领。")
            record = {
                "version": 1, "event": "finish", "claim_id": previous["claim_id"],
                "session": args.session, "at": timestamp(), "issue": previous["issue"],
                "branch": branch, "worktree": str(root), "pr": args.pr, "summary": args.summary,
            }
        append_record(directory, record)
        print(f"{args.command} 已记录：{args.session} #{record['issue']}（仅本地，不提交或推送）")


def positive_number(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("编号必须大于 0")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("path", help="显示当前仓库共享记录目录")
    status = commands.add_parser("status", help="查看所有会话当前认领")
    status.add_argument("--json", action="store_true")
    history = commands.add_parser("history", help="查看本地追加流水（JSONL）")
    history.add_argument("--session")
    claim = commands.add_parser("claim", help="原子检查并认领工作范围")
    claim.add_argument("--session", required=True)
    claim.add_argument("--issue", type=positive_number, required=True)
    claim.add_argument("--file", action="append", required=True)
    claim.add_argument("--summary", required=True)
    finish = commands.add_parser("finish", help="完成/中止自己的任务并释放范围")
    finish.add_argument("--session", required=True)
    finish.add_argument("--pr", type=positive_number)
    finish.add_argument("--summary", required=True)
    try:
        run(parser.parse_args())
    except (CoordinationError, OSError) as error:
        print(f"coordination: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
