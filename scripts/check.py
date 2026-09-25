#!/usr/bin/env python3
"""The per-round checks, each run once, logged whole, summarized short.

A round of work ends on the same six commands every time -- `cargo fmt
--check`, `cargo clippy`, the suite under `cargo nextest`, the doctests,
the scripts' own `unittest` and `repoint.py` -- and a session reading their raw
output pipes it through `grep` and `head` to find the lines that matter, then
re-runs the suite when the pipe cut what it needed. This runs each command
once, writes its whole output to a log under `runs/check/`, and prints a
summary of fixed shape: one line a check carrying the tool's own result line,
and beneath a failing check the lines naming what failed. Everything else is in
the log the summary names, so nothing needs a second run to be read.

**The run is stamped with the tree it tested**: a git tree id covering every
file `git add -A` would stage -- tracked and untracked, ignored ones excluded
-- computed through a scratch index, so the real one is never touched. On a
clean tree it is `HEAD^{tree}`, and once the tree is committed it is that
commit's tree, which is what ties a commit to the run that tested it. The
stamp is taken before the checks and again after; a tree that changed in
between is reported and the run verifies neither.

**`--verify` reuses a passing run of this tree** instead of re-running it, so
a second reader of a round's result -- the orchestrator that did not do the
work -- checks the evidence rather than paying for it again. Only the newest
run of the tree counts, so a failure is never hidden behind an earlier pass,
and only a run of this same list of commands. With no such run it runs the
checks.

**A red repoint meter is not a failure.** It says the record has outgrown its
last blind read and a repoint is due, which is a round of its own
(`.claude/skills/go/SKILL.md`); the summary says so, and the run passes. A cap
`repoint.py` names fails it like any other check.

**Clippy passes only with no diagnostic**: its exit status is zero over
warnings, and a warning left for the next round is one nobody fixes.

Usage:

    mise run check [--verify]
    cd scripts && uv run check.py [--verify]
    cd scripts && uv run python -m unittest test_check
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence, TextIO

REPO = Path(__file__).resolve().parent.parent
#: Where the runs go, relative to the repository; gitignored with `runs/`.
RUNS = Path("runs") / "check"
#: Lines listed under a failing check before the rest is left to its log.
DETAIL_LINES = 12


@dataclass(frozen=True)
class Outcome:
    """What one check's output says, read from its log."""

    status: str  # "ok", "FAILED", or "meter" (repoint's meter alone is red)
    headline: str
    details: tuple[str, ...] = ()


@dataclass(frozen=True)
class Check:
    name: str
    argv: tuple[str, ...]
    #: Relative to the repository root.
    cwd: str
    summarize: Callable[[str, int], Outcome]

    def key(self) -> list:
        return [self.name, list(self.argv), self.cwd]


def _tail(text: str, n: int = 5) -> tuple[str, ...]:
    lines = [line.rstrip() for line in text.splitlines() if line.strip()]
    return tuple(lines[-n:])


def _failed_or_ok(exit_code: int) -> str:
    return "ok" if exit_code == 0 else "FAILED"


FMT_DIFF_RE = re.compile(r"^Diff in (.+?)(?::\d+:| at line \d+:)\s*$")


def summarize_fmt(text: str, exit_code: int) -> Outcome:
    files: list[str] = []
    for line in text.splitlines():
        m = FMT_DIFF_RE.match(line)
        if m and m.group(1) not in files:
            files.append(m.group(1))
    if exit_code == 0:
        return Outcome("ok", "formatted")
    if files:
        return Outcome("FAILED", f"{len(files)} files would be reformatted", tuple(files))
    return Outcome("FAILED", "cargo fmt --check failed", _tail(text))


CLIPPY_DIAGNOSTIC_RE = re.compile(r"^(warning|error)(\[\w+\])?: ")
CLIPPY_TALLY_RE = re.compile(
    r"^(warning: .* generated \d+ warnings?|error: could not compile|error: aborting due to)"
)
CLIPPY_LOCATION_RE = re.compile(r"^\s*--> (\S+)")


def summarize_clippy(text: str, exit_code: int) -> Outcome:
    lines = text.splitlines()
    warnings = errors = 0
    details: list[str] = []
    for i, line in enumerate(lines):
        if not CLIPPY_DIAGNOSTIC_RE.match(line) or CLIPPY_TALLY_RE.match(line):
            continue
        if line.startswith("warning"):
            warnings += 1
        else:
            errors += 1
        where = next(
            (m.group(1) for nxt in lines[i + 1 : i + 4] if (m := CLIPPY_LOCATION_RE.match(nxt))),
            None,
        )
        detail = f"{line.strip()}  ({where})" if where else line.strip()
        if detail not in details:
            details.append(detail)
    headline = f"{warnings} warnings, {errors} errors"
    if exit_code == 0 and warnings == 0 and errors == 0:
        return Outcome("ok", headline)
    return Outcome("FAILED", headline, tuple(details) or _tail(text))


NEXTEST_SUMMARY_RE = re.compile(r"^\s*Summary \[")


def summarize_nextest(text: str, exit_code: int) -> Outcome:
    lines = text.splitlines()
    at = next((i for i, line in enumerate(lines) if NEXTEST_SUMMARY_RE.match(line)), None)
    if at is None:
        errors = tuple(line.strip() for line in lines if line.startswith("error"))
        return Outcome("FAILED", "nextest printed no Summary line", errors or _tail(text))
    # What follows nextest's Summary line is its list of what failed.
    after = tuple(
        line.strip()
        for line in lines[at + 1 :]
        if line.strip() and not line.startswith("error: test run failed")
    )
    return Outcome(_failed_or_ok(exit_code), lines[at].strip(), after)


DOC_TARGET_RE = re.compile(r"^\s*Doc-tests ")
DOC_RESULT_RE = re.compile(r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed")
DOC_FAILED_RE = re.compile(r"^test (.+) \.\.\. FAILED$")


def summarize_doctest(text: str, exit_code: int) -> Outcome:
    targets = passed = failed = 0
    details: list[str] = []
    for line in text.splitlines():
        if DOC_TARGET_RE.match(line):
            targets += 1
        elif m := DOC_RESULT_RE.match(line):
            passed += int(m.group(1))
            failed += int(m.group(2))
        elif m := DOC_FAILED_RE.match(line.strip()):
            details.append(m.group(1))
    headline = f"{targets} doc-test targets: {passed} passed; {failed} failed"
    if exit_code == 0:
        return Outcome("ok", headline)
    if not details:
        details = [line.strip() for line in text.splitlines() if line.startswith("error")]
    return Outcome("FAILED", headline, tuple(details) or _tail(text))


UNITTEST_RAN_RE = re.compile(r"^Ran \d+ tests? in ")
UNITTEST_VERDICT_RE = re.compile(r"^(OK|FAILED)( \(.*\))?$")
UNITTEST_FAILURE_RE = re.compile(r"^(FAIL|ERROR): ")


def summarize_unittest(text: str, exit_code: int) -> Outcome:
    ran = verdict = None
    details: list[str] = []
    for line in text.splitlines():
        if UNITTEST_RAN_RE.match(line):
            ran = line.strip()
        elif UNITTEST_VERDICT_RE.match(line.strip()):
            verdict = line.strip()
        elif UNITTEST_FAILURE_RE.match(line) and line.strip() not in details:
            details.append(line.strip())
    if ran is None or verdict is None:
        return Outcome("FAILED", "unittest printed no result", _tail(text))
    headline = f"{ran}; {verdict}"
    if exit_code == 0:
        return Outcome("ok", headline)
    return Outcome("FAILED", headline, tuple(details) or _tail(text))


#: `repoint.py`'s one problem that is its meter rather than a cap
#: (`meter_problems`); `test_check` pins it against the real script's output.
REPOINT_METER_RE = re.compile(r"^!! the live record has grown -?\d+ lines since \S+; repoint$")


def summarize_repoint(text: str, exit_code: int) -> Outcome:
    lines = [line.rstrip() for line in text.splitlines()]
    growth = next((line for line in lines if line.startswith("since ")), None)
    verdict = next((line for line in lines if line.startswith("repoint: ")), None)
    problems = tuple(line for line in lines if line.startswith("!! "))
    headline = "; ".join(x for x in (growth, verdict) if x) or "repoint.py printed no result"
    if exit_code == 0:
        return Outcome("ok", headline)
    if problems and all(REPOINT_METER_RE.match(p) for p in problems):
        return Outcome("meter", headline, problems)
    return Outcome("FAILED", headline, problems or _tail(text))


CHECKS: tuple[Check, ...] = (
    Check("fmt", ("cargo", "fmt", "--check"), ".", summarize_fmt),
    Check("clippy", ("cargo", "clippy", "--workspace", "--all-targets"), ".", summarize_clippy),
    Check(
        "nextest",
        ("cargo", "nextest", "run", "--workspace", "--no-fail-fast"),
        ".",
        summarize_nextest,
    ),
    Check(
        "doctest",
        ("cargo", "test", "--workspace", "--doc", "--no-fail-fast"),
        ".",
        summarize_doctest,
    ),
    Check("unittest", ("uv", "run", "python", "-m", "unittest"), "scripts", summarize_unittest),
    Check("repoint", ("uv", "run", "repoint.py"), "scripts", summarize_repoint),
)


def _git(repo: Path, *args: str, env: dict[str, str] | None = None) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, env=env, check=True, capture_output=True, text=True
    ).stdout.strip()


def tree_stamp(repo: Path) -> str:
    """The git tree id of the working tree as `git add -A` would stage it."""
    index = Path(_git(repo, "rev-parse", "--path-format=absolute", "--git-path", "index"))
    with tempfile.TemporaryDirectory() as tmp:
        scratch = Path(tmp) / "index"
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env["GIT_INDEX_FILE"] = str(scratch)
        if index.exists():
            # A copy keeps the real index's stat cache, so unchanged files are
            # not re-hashed.
            shutil.copyfile(index, scratch)
        _git(repo, "add", "-A", env=env)
        return _git(repo, "write-tree", env=env)


def head_tree(repo: Path) -> tuple[str | None, str | None]:
    try:
        return _git(repo, "rev-parse", "HEAD"), _git(repo, "rev-parse", "HEAD^{tree}")
    except subprocess.CalledProcessError:
        return None, None


def _indent(details: Sequence[str], log: str) -> list[str]:
    shown = [f"    {d}" for d in details[:DETAIL_LINES]]
    if len(details) > DETAIL_LINES:
        shown.append(f"    ... {len(details) - DETAIL_LINES} more in {log}")
    return shown


def render_header(record: dict) -> str:
    head = record["head"][:9] if record["head"] else "no commit"
    dirty = "" if record["tree"] == record["head_tree"] else " with uncommitted changes"
    return f"check: tree {record['tree'][:12]}, HEAD {head}{dirty}; logs in {record['dir']}/"


def render_check(entry: dict) -> str:
    line = f"{entry['name']:<9}{entry['status']:<7}{entry['seconds']:>5.0f}s  {entry['headline']}"
    return "\n".join([line, *_indent(entry["details"], entry["log"])])


def render_footer(record: dict) -> str:
    failed = [c["name"] for c in record["checks"] if c["status"] == "FAILED"]
    parts = [f"check: FAILED ({', '.join(failed)})" if failed else "check: passed"]
    if record["tree_after"] != record["tree"]:
        parts = [
            f"check: FAILED, the tree changed while the checks ran ({record['tree'][:12]} -> "
            f"{record['tree_after'][:12]}), so this run verifies neither",
            *([f"failed: {', '.join(failed)}"] if failed else []),
        ]
    if any(c["status"] == "meter" for c in record["checks"]):
        parts.append("repoint's meter is red, so a repoint is due")
    return "; ".join(parts)


def run(repo: Path, checks: Sequence[Check] = CHECKS, out: TextIO = sys.stdout) -> int:
    started = time.time()
    tree = tree_stamp(repo)
    head, htree = head_tree(repo)
    stem = f"{time.strftime('%Y%m%d-%H%M%S', time.localtime(started))}-{tree[:12]}"
    (repo / RUNS).mkdir(parents=True, exist_ok=True)
    rel = RUNS / stem
    for n in range(2, 100):
        try:
            (repo / rel).mkdir()
            break
        except FileExistsError:
            rel = RUNS / f"{stem}-{n}"
    run_dir = repo / rel
    record: dict = {
        "dir": str(rel),
        "tree": tree,
        "head": head,
        "head_tree": htree,
        "started": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(started)),
        "epoch": started,
        "checks": [],
    }
    print(render_header(record), file=out, flush=True)
    env = dict(os.environ, CARGO_TERM_COLOR="never", NO_COLOR="1")
    for check in checks:
        log = rel / f"{check.name}.log"
        t0 = time.monotonic()
        with open(repo / log, "w", encoding="utf-8") as sink:
            try:
                code = subprocess.run(
                    check.argv,
                    cwd=repo / check.cwd,
                    env=env,
                    stdin=subprocess.DEVNULL,
                    stdout=sink,
                    stderr=subprocess.STDOUT,
                ).returncode
            except FileNotFoundError as exc:
                print(exc, file=sink)
                code = 127
        text = (repo / log).read_text(encoding="utf-8", errors="replace")
        outcome = check.summarize(text, code)
        entry = {
            "name": check.name,
            "argv": list(check.argv),
            "cwd": check.cwd,
            "exit": code,
            "seconds": time.monotonic() - t0,
            "status": outcome.status,
            "headline": outcome.headline,
            "details": list(outcome.details),
            "log": str(log),
        }
        record["checks"].append(entry)
        print(render_check(entry), file=out, flush=True)
    record["tree_after"] = tree_stamp(repo)
    record["seconds"] = time.time() - started
    record["passed"] = record["tree_after"] == tree and all(
        c["status"] != "FAILED" for c in record["checks"]
    )
    footer = render_footer(record)
    print(footer, file=out, flush=True)
    summary = [render_header(record), *(render_check(c) for c in record["checks"]), footer]
    (run_dir / "summary.txt").write_text("\n".join(summary) + "\n", encoding="utf-8")
    (run_dir / "record.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return 0 if record["passed"] else 1


def newest_run_of(repo: Path, tree: str, checks: Sequence[Check]) -> dict | None:
    """The newest recorded run of these checks over this tree, passed or not."""
    root = repo / RUNS
    if not root.is_dir():
        return None
    wanted = [c.key() for c in checks]
    newest = None
    for run_dir in root.iterdir():
        try:
            record = json.loads((run_dir / "record.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        ran = [[c["name"], c["argv"], c["cwd"]] for c in record.get("checks", [])]
        if record.get("tree") != tree or ran != wanted:
            continue
        if newest is None or record.get("epoch", 0) > newest.get("epoch", 0):
            newest = record
    return newest


def verify(repo: Path, checks: Sequence[Check] = CHECKS, out: TextIO = sys.stdout) -> int:
    record = newest_run_of(repo, tree_stamp(repo), checks)
    if record is None or not record.get("passed"):
        why = "no run of this tree" if record is None else f"{record['dir']} did not pass"
        print(f"check: {why}; running every check", file=out, flush=True)
        return run(repo, checks, out)
    print(
        f"check: reusing {record['dir']} ({record['started']}), the newest run of this tree",
        file=out,
    )
    print((repo / record["dir"] / "summary.txt").read_text(encoding="utf-8"), end="", file=out)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="reuse the newest run of this tree if it passed; run every check otherwise",
    )
    args = parser.parse_args(argv)
    return (verify if args.verify else run)(REPO)


if __name__ == "__main__":
    raise SystemExit(main())
