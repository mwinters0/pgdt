#!/usr/bin/env python3
"""The per-round checks, each run once, logged whole, summarized short.

A round of work ends on the same six commands every time -- `cargo fmt
--check`, `cargo clippy`, the suite under `cargo nextest`, the doctests,
the scripts' own `unittest` and `repoint.py` -- and a session reading their raw
output pipes it through `grep` and `head` to find the lines that matter, then
re-runs the suite when the pipe cut what it needed. This runs each command
once, writes its whole output to a log under `runs/check/`, and prints a
summary of fixed shape: one line a check carrying its result -- the tool's own
result line where it prints one, a count read from its output where it does
not -- and beneath a failing check the lines naming what failed. Everything else is in
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
work -- checks the evidence rather than paying for it again. Only a run of
this same list of commands counts, and of those only the tree's newest, so a
failure is never hidden behind an earlier pass of the list; an `--affected`
verify counts a run of every check too. With no such run it runs the checks.

**A red repoint meter is not a failure.** It says the record has outgrown its
last blind read and a repoint is due, which is a round of its own
(`.claude/skills/go/SKILL.md`); the summary says so, and the run passes. A cap
`repoint.py` names fails it like any other check.

**Clippy passes only with no diagnostic**: its exit status is zero over
warnings, and a warning left for the next round is one nobody fixes.

**`--affected` runs the cargo checks the change can reach**, the change being
every path whose content differs between `HEAD`'s tree and the stamped one.
The scripts' `unittest` and `repoint.py` always run, being cheap and the ones
that read the docs. A changed path runs the targets reading it, and what the
first of the other three rules it meets gives it:

- **Read by a test target** -- a string literal in one of its sources, taken
  relative to its package and to its file, names the path or a directory
  above it -- runs that target (`binary_id` in nextest's terms), in any
  package. A test's working directory is its package's, and a literal is
  truncated at the first `{`, so `format!("../fixtures/{major}")` reads all
  of `fixtures/`.
- **Inside a workspace member** runs that package whole; outside its
  `tests/`, `benches/` and `examples/` it runs every member depending on it,
  transitively, too. Those packages are the ones formatted and linted, and
  doc-tested where they have a library, as is a library whose own tests read
  a changed path.
- **`docs/`, `.claude/` or a Markdown file** runs nothing. No Rust target
  names such a path or reads `docs/` or `.claude/` whole, a Markdown file in
  a data directory documenting it; the one Rust test reaching one does so
  through a `test_*` module the scripts' `unittest` runs already, and the
  other scripts Rust tests run read none (`test_check` pins the first two).
- **Anything else no test reads** -- the workspace manifest, the lockfile,
  `vendor/`, a tool's config -- runs every check, as without the flag.

A change touching only the third kind runs no cargo at all, not even
`cargo metadata`. A phase wrap runs every check
(`.claude/skills/process/SKILL.md`, "Wrapping a phase").

Usage:

    mise run check [--affected] [--verify]
    cd scripts && uv run check.py [--affected] [--verify]
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


#: Paths taken as read by no Rust target: `test_check` asserts that no literal
#: names one, nor the repository, `docs/` or `.claude/` whole.
UNTESTED_RE = re.compile(r"^(docs/|\.claude/)|\.md$")
#: The directories of a package whose change runs none of its dependents.
PACKAGE_LOCAL = ("tests/", "benches/", "examples/")
#: Cargo target kinds a library's tests run under, in nextest's binary id.
LIB_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}


@dataclass(frozen=True)
class Target:
    """A target nextest runs tests from, with the files it compiles."""

    binary_id: str
    kind: str  # "lib", "bin" or "test"
    sources: tuple[str, ...]  # repo-relative


@dataclass(frozen=True)
class Package:
    name: str
    dir: str  # repo-relative, no trailing slash
    #: Workspace members it depends on, of any dependency kind.
    deps: frozenset[str]
    targets: tuple[Target, ...]

    @property
    def has_lib(self) -> bool:
        return any(t.kind == "lib" for t in self.targets)


_MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.M)


def module_files(repo: Path, root: str) -> list[str]:
    """`root` and every file its `mod name;` lines reach at the conventional
    paths, repo-relative; a `#[path]`, an attribute on the same line and a
    declaration inside an inline module are not followed."""
    out: list[str] = []
    pending = [(root, True)]
    while pending:
        rel, is_mod_root = pending.pop()
        if rel in out:
            continue
        out.append(rel)
        path = repo / rel
        base = path.parent if is_mod_root else path.with_suffix("")
        for name in _MOD_RE.findall(path.read_text(encoding="utf-8", errors="replace")):
            for cand, root_like in ((base / f"{name}.rs", False), (base / name / "mod.rs", True)):
                if cand.is_file():
                    pending.append((cand.relative_to(repo).as_posix(), root_like))
                    break
    return out


_CHAR_RE = re.compile(r"'(?:\\u\{[0-9a-fA-F]+\}|\\x[0-9a-fA-F]{2}|\\.|[^\\'])'")
_RAW_RE = re.compile(r'b?r(#*)"')
_PLAIN_STRING_RE = re.compile(r'"((?:[^"\\]|\\.)*)"', re.S)


def string_literals(src: str) -> list[str]:
    """The string literals in Rust source, line doc comments' included
    (doctests are code), block and ordinary comments' not."""
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end < 0 else end
            if src.startswith(("///", "//!"), i):
                out.extend(m.group(1) for m in _PLAIN_STRING_RE.finditer(src, i, end))
            i = end
        elif src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif (m := _RAW_RE.match(src, i)) and not (i and (src[i - 1].isalnum() or src[i - 1] == "_")):
            close = '"' + m.group(1)
            end = src.find(close, m.end())
            end = n if end < 0 else end
            out.append(src[m.end() : end])
            i = end + len(close)
        elif c == '"':
            m = _PLAIN_STRING_RE.match(src, i)
            if not m:
                break
            out.append(m.group(1))
            i = m.end()
        elif c == "'":
            m = _CHAR_RE.match(src, i)
            i = m.end() if m else i + 1  # a lifetime or a label
        else:
            i += 1
    return out


def read_prefixes(repo: Path, package_dir: str, source: str) -> set[str]:
    """The repo-relative paths a source's literals can name: each literal with no
    whitespace, cut at its first `{`, taken from the package's directory (a
    test's working directory, and `CARGO_MANIFEST_DIR`) and from the file's own
    (`include_str!`). `.` is the whole repository."""
    text = (repo / source).read_text(encoding="utf-8", errors="replace")
    out: set[str] = set()
    for lit in string_literals(text):
        lit = lit.split("{", 1)[0]
        if not lit or any(ch.isspace() for ch in lit) or "\\" in lit or lit.startswith("/"):
            continue
        for base in (package_dir, str(Path(source).parent)):
            norm = os.path.normpath(os.path.join(base, lit))
            if norm != ".." and not norm.startswith("../"):
                out.add(Path(norm).as_posix())
    return out


def reads(prefix: str, path: str) -> bool:
    """Whether a literal naming `prefix` reads `path`. A path no test reads is
    read only by a literal naming it: one in a directory a test reads, like a
    runtime root's `README.md`, documents the directory."""
    if UNTESTED_RE.search(path):
        return path == prefix
    return prefix == "." or path == prefix or path.startswith(prefix + "/")


def workspace(repo: Path) -> list[Package]:
    """The workspace members and their test targets, from `cargo metadata`."""
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=repo, check=True, capture_output=True, text=True,
        ).stdout
    )
    root = Path(meta["workspace_root"])
    members = set(meta["workspace_members"])
    dirs = {
        p["name"]: Path(p["manifest_path"]).parent.relative_to(root).as_posix()
        for p in meta["packages"]
        if p["id"] in members
    }
    by_dir = {d: name for name, d in dirs.items()}
    packages = []
    for p in meta["packages"]:
        if p["id"] not in members:
            continue
        name, pdir = p["name"], dirs[p["name"]]
        deps = set()
        for d in p["dependencies"]:
            if d.get("path"):
                rel = Path(d["path"]).resolve().relative_to(root.resolve()).as_posix()
                if rel in by_dir:
                    deps.add(by_dir[rel])
        targets = []
        for t in p["targets"]:
            src = Path(t["src_path"]).relative_to(root).as_posix()
            kinds = set(t["kind"])
            if kinds & LIB_KINDS or "bin" in kinds:
                kind = "lib" if kinds & LIB_KINDS else "bin"
                src_dir = Path(src).parent
                sources = sorted(f.relative_to(repo).as_posix() for f in (repo / src_dir).rglob("*.rs"))
                bid = name if kind == "lib" else f"{name}::bin/{t['name']}"
            elif "test" in kinds:
                kind, sources, bid = "test", module_files(repo, src), f"{name}::{t['name']}"
            else:
                continue  # benches, examples and build scripts run no tests
            targets.append(Target(bid, kind, tuple(sources)))
        packages.append(Package(name, pdir, frozenset(deps - {name}), tuple(targets)))
    return packages


#: Binaries a note names one by one; past it, it counts them by package, and
#: nextest's argument in the run's `record.json` names each.
NOTE_BINARIES = 6


def _paths(n: int) -> str:
    return "1 path" if n == 1 else f"{n} paths"


@dataclass(frozen=True)
class Plan:
    """What `--affected` runs for one tree, and the line saying why."""

    checks: tuple[Check, ...]
    note: str


def changed_paths(repo: Path, head_tree: str, tree: str) -> list[str]:
    out = _git(repo, "diff", "--no-renames", "--name-only", head_tree, tree)
    return [line for line in out.splitlines() if line]


def plan_affected(
    repo: Path,
    tree: str,
    head_tree: str | None,
    load: Callable[[Path], list[Package]] = workspace,
) -> Plan:
    if head_tree is None:
        return Plan(CHECKS, "affected: no commit to compare with, so every check runs")
    return plan_changes(repo, changed_paths(repo, head_tree, tree), load)


def plan_changes(
    repo: Path, changed: Sequence[str], load: Callable[[Path], list[Package]] = workspace
) -> Plan:
    always = tuple(c for c in CHECKS if c.name in ("unittest", "repoint"))
    if not changed:
        return Plan(always, "affected: nothing changed since HEAD, so no cargo check runs")
    if all(UNTESTED_RE.search(p) for p in changed):
        return Plan(
            always,
            f"affected: {_paths(len(changed))} changed since HEAD, none read by a test, "
            "so no cargo check runs",
        )
    packages = load(repo)
    by_name = {p.name: p for p in packages}
    prefixes = {
        (p.name, t): set().union(*(read_prefixes(repo, p.dir, s) for s in t.sources))
        for p in packages
        for t in p.targets
    }
    whole, roots, local = None, set(), set()
    reader_ids: set[tuple[str, str]] = set()
    for path in changed:
        found = {
            (pkg, t.binary_id)
            for (pkg, t), pre in prefixes.items()
            if any(reads(x, path) for x in pre)
        }
        reader_ids |= found
        if UNTESTED_RE.search(path):
            continue
        home = next((p for p in packages if path.startswith(p.dir + "/")), None)
        if home is not None:
            rest = path[len(home.dir) + 1 :]
            (local if rest.startswith(PACKAGE_LOCAL) else roots).add(home.name)
        elif not found and whole is None:
            whole = path
    if whole is not None:
        return Plan(
            CHECKS, f"affected: {whole} is in no crate and read by no test, so every check runs"
        )
    closure = set(roots)
    grew = True
    while grew:
        more = {p.name for p in packages if p.deps & closure} - closure
        closure |= more
        grew = bool(more)
    whole_pkgs = sorted(closure | local)
    binaries = sorted((pkg, bid) for pkg, bid in reader_ids if pkg not in whole_pkgs)
    doc_pkgs = sorted(
        {n for n in whole_pkgs if by_name[n].has_lib}
        | {pkg for pkg, bid in binaries if bid == pkg}
    )
    checks: list[Check] = []
    p_args = tuple(a for n in whole_pkgs for a in ("-p", n))
    if whole_pkgs:
        checks.append(Check("fmt", ("cargo", "fmt", "--check", *p_args), ".", summarize_fmt))
        checks.append(
            Check("clippy", ("cargo", "clippy", "--all-targets", *p_args), ".", summarize_clippy)
        )
    if whole_pkgs or binaries:
        run_pkgs = sorted(set(whole_pkgs) | {pkg for pkg, _ in binaries})
        argv = ["cargo", "nextest", "run", "--no-fail-fast"]
        argv += [a for n in run_pkgs for a in ("-p", n)]
        if binaries:
            terms = [f"package(={n})" for n in whole_pkgs] + [f"binary_id(={b})" for _, b in binaries]
            argv += ["-E", " | ".join(terms)]
        checks.append(Check("nextest", tuple(argv), ".", summarize_nextest))
    if doc_pkgs:
        argv = ("cargo", "test", "--doc", "--no-fail-fast", *(a for n in doc_pkgs for a in ("-p", n)))
        checks.append(Check("doctest", argv, ".", summarize_doctest))
    parts = [f"{_paths(len(changed))} changed since HEAD"]
    if whole_pkgs:
        parts.append(f"packages {', '.join(whole_pkgs)}")
    if len(binaries) > NOTE_BINARIES:
        per = {pkg: sum(1 for p, _ in binaries if p == pkg) for pkg, _ in binaries}
        counts = ", ".join(f"{pkg} {k}" for pkg, k in per.items())
        parts.append(f"{len(binaries)} binaries reading a changed path ({counts})")
    elif binaries:
        parts.append(f"binaries reading a changed path {', '.join(b for _, b in binaries)}")
    return Plan((*checks, *always), "affected: " + "; ".join(parts))


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


def run(
    repo: Path,
    checks: Sequence[Check] = CHECKS,
    out: TextIO = sys.stdout,
    note: str | None = None,
    tree: str | None = None,
) -> int:
    """Run `checks`, stamped with `tree` (by default the tree as it is now), and
    print `note` beneath the header."""
    started = time.time()
    tree = tree or tree_stamp(repo)
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
        "note": note,
        "checks": [],
    }
    preface = [render_header(record), *([note] if note else [])]
    print("\n".join(preface), file=out, flush=True)
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
    summary = [*preface, *(render_check(c) for c in record["checks"]), footer]
    (run_dir / "summary.txt").write_text("\n".join(summary) + "\n", encoding="utf-8")
    (run_dir / "record.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    return 0 if record["passed"] else 1


def newest_run_of(
    repo: Path, tree: str, checks: Sequence[Check], also: Sequence[Sequence[Check]] = ()
) -> dict | None:
    """The newest recorded run over this tree of these checks, or of a list in
    `also`, passed or not."""
    root = repo / RUNS
    if not root.is_dir():
        return None
    wanted = [[c.key() for c in cs] for cs in (checks, *also)]
    newest = None
    for run_dir in root.iterdir():
        try:
            record = json.loads((run_dir / "record.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        ran = [[c["name"], c["argv"], c["cwd"]] for c in record.get("checks", [])]
        if record.get("tree") != tree or ran not in wanted:
            continue
        if newest is None or record.get("epoch", 0) > newest.get("epoch", 0):
            newest = record
    return newest


def verify(
    repo: Path,
    checks: Sequence[Check] = CHECKS,
    out: TextIO = sys.stdout,
    note: str | None = None,
    tree: str | None = None,
    also: Sequence[Sequence[Check]] = (),
) -> int:
    """Reuse the newest run of this tree if it passed, else `run`; a run of a
    list in `also` -- the whole list, for an `--affected` verify -- counts too."""
    tree = tree or tree_stamp(repo)
    record = newest_run_of(repo, tree, checks, also)
    if record is None or not record.get("passed"):
        why = "no run of this tree" if record is None else f"{record['dir']} did not pass"
        print(f"check: {why}; running the checks", file=out, flush=True)
        return run(repo, checks, out, note, tree)
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
        help="reuse the newest run of this tree if it passed; run the checks otherwise",
    )
    parser.add_argument(
        "--affected",
        action="store_true",
        help="run only the cargo checks the change since HEAD can reach",
    )
    args = parser.parse_args(argv)
    if not args.affected:
        return (verify if args.verify else run)(REPO)
    tree = tree_stamp(REPO)
    plan = plan_affected(REPO, tree, head_tree(REPO)[1])
    if args.verify:
        return verify(REPO, plan.checks, note=plan.note, tree=tree, also=(CHECKS,))
    return run(REPO, plan.checks, note=plan.note, tree=tree)


if __name__ == "__main__":
    raise SystemExit(main())
