#!/usr/bin/env python3
"""The per-round checks, each run once, logged whole, summarized short.

A round of work ends on the same nine commands every time -- `cargo fmt
--check`, `cargo clippy`, each library linted alone, the suite under `cargo
nextest`, the doctests, `module_graph.py`, the scripts' own `unittest`,
`repoint.py` and `file_limits.py` -- and a session reading their raw
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
this same list of commands is reused, the newest of them, and only while no
run of the tree since, of any list, failed: a failure is never hidden behind
an earlier pass, a flaky test failing an `--affected` run included. An
`--affected` verify reuses a run of every check too. With no such run it
runs the checks.

**A red repoint meter is not a failure.** It says the record has outgrown its
last blind read and a repoint is due, which is a round of its own
(`.claude/skills/go/SKILL.md`); the summary says so, and the run passes. A cap
`repoint.py` names fails it like any other check. **So is a red seams
meter** (`file_limits.py`): a file past a hard limit says a seams round is due
(`.claude/skills/seams/SKILL.md`), and only a stale exemption fails the check.

**The module graph fails on a cycle neither kept nor owed**
(`module_graph.py`): it is the one-way rule between modules, a check rather
than a meter, and its doc links against the edges are a count that never
fails.

**Clippy passes only with no diagnostic**: its exit status is zero over
warnings, and a warning left for the next round is one nobody fixes.

**Each library is linted alone too** (`--alone`, `run_alone`): one cargo run
per member holding a library, read off `cargo metadata`, as an embedder builds
it and then with every target. One workspace build unifies features across
members -- `pgdt` turns on `pgdt-io`'s `http`, a dev-dependency a crate's
`test-support` -- so code only a feature reaches is reachable there, and dead in
the library an embedder builds without it; a warning only a build alone meets
is fixed by gating that code on the feature reaching it.

**So is every other build the project makes or names to an embedder**
(`FEATURE_BUILDS`), each a named feature set linted at both of a library's
builds: the instrument over the workspace, `pgdt`'s allocator legs, `pgdt-gzip-seek`'s
engine legs, `datafusion-pgdump` with `http`. A feature no linted build turns
on compiles nowhere a warning would surface, so `test_check` asserts off
`cargo metadata` that every feature a member declares is turned on by one --
the workspace's, a library's alone or a set -- or is exempted in
`UNLINTED_FEATURES` with its reason. The sets are named rather than generated
(`cargo hack --each-feature`), which refuses the two exclusive choices
(`pgdt-gzip-seek`'s inflater, `pgdt`'s allocator) until told them in its own syntax
and then lints combinations nobody builds.

**The cargo checks run in the release image** (`release.py run`), as the
invoking user under its memory limit and on every CPU but one physical core's
(`release.py`, `RESERVE_CORES`): fmt, clippy, the libraries alone, the
suite, the doctests and the module graph, whose rust-analyzer builds and runs the workspace's build
scripts. A whole-workspace test build compiles and links dozens of binaries at
once, and on a host without swap one outgrowing memory stalls the whole machine
rather than failing; under the limit the kernel kills a compiler or a linker
inside the container, and the check fails naming it. The image is where CI's
suite runs too (`docs/design/decisions.md`, "D104"), so the round builds what
CI builds, in the release state's target directory (`target/release-image/`),
never the host's `target/`, which a session's own `cargo` keeps. The scripts'
checks stay on the host, building nothing. `PGDT_RELEASE_CONTAINER` and
`PGDT_RELEASE_MEMORY` name the runtime and the limit, and
`PGDT_RELEASE_RESERVE_CORES` how many cores it leaves (`release.py`).

**The doctests run only for a library holding one** (`has_doctest`, by
rustdoc's own reading of a code block), because `cargo test --doc` compiles
every library it is given once more just to find nothing to run; with none,
the check is left out. The reading errs only toward running.

**`--affected` runs the cargo checks the change can reach**, the change being
every path whose content differs between `HEAD`'s tree and the stamped one.
The scripts' `unittest`, `repoint.py` and `file_limits.py` always run, being
cheap and the ones that read the docs and the tree's shape. A changed path runs the targets reading it, and what the
first of the other three rules it meets gives it:

- **Read by a test target** -- a string literal in one of its sources, taken
  relative to its package and to its file, names the path or a directory
  above it -- runs that target (`binary_id` in nextest's terms), in any
  package. A test's working directory is its package's, and a templated
  literal names the directory it fills, so `format!("../fixtures/{major}")`
  reads all of `fixtures/`; a bare `.` names nothing (`literal_path`). A
  library only dev-dependencies reach, a test-support crate, reads for every
  target of each package depending on it, any of which may call what reads.
- **Inside a workspace member** runs that package whole; outside its
  `tests/`, `benches/` and `examples/` it runs every member depending on it,
  transitively, too. Those packages are the ones formatted and linted, each
  library among them alone too and each feature set of theirs -- every set
  of a workspace-wide build being every package's -- and doc-tested where
  they have a library, as
  is a library whose own tests read a changed path; and the module graph runs,
  whole, since its index is.
- **A comment, inside a workspace member** -- a `.rs` file in both trees
  whose `code_of` is unchanged, a doc comment's fenced doctest counting as
  code -- formats and lints that package by itself, alone if a library and
  under its feature sets, and runs none of its tests, doctests or dependents: clippy reads doc
  comments, and no build reads the rest. A target reading the file still
  runs, by the first rule.
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
    cd scripts && uv run check.py --alone [LIBRARY|SET ...]   # the alone check by itself; cargo on the host
    cd scripts && uv run python -m unittest test_check
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Collection, Sequence, TextIO

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


#: How a check runs in the release image: `release.py run`, from the
#: repository root, naming the directory the command runs from there.
IMAGE_RUN = ("python3", "scripts/release.py", "run")


@dataclass(frozen=True)
class Check:
    name: str
    argv: tuple[str, ...]
    #: Relative to the repository root.
    cwd: str
    summarize: Callable[[str, int], Outcome]
    #: Whether it runs in the release image rather than on the host.
    image: bool = False

    def command(self) -> tuple[tuple[str, ...], str]:
        """The argv the host runs, and the directory it runs it from."""
        if not self.image:
            return self.argv, self.cwd
        return (*IMAGE_RUN, f"--cwd={self.cwd}", "--", *self.argv), "."

    def key(self) -> list:
        argv, cwd = self.command()
        return [self.name, list(argv), cwd]


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


def _diagnostics(lines: Sequence[str]) -> list[tuple[int, str, str | None]]:
    """Each diagnostic clippy printed: its line's index, the line, and the
    location beneath it, if any; a tally line is none."""
    out = []
    for i, line in enumerate(lines):
        if not CLIPPY_DIAGNOSTIC_RE.match(line) or CLIPPY_TALLY_RE.match(line):
            continue
        where = next(
            (m.group(1) for nxt in lines[i + 1 : i + 4] if (m := CLIPPY_LOCATION_RE.match(nxt))),
            None,
        )
        out.append((i, line.strip(), where))
    return out


def summarize_clippy(text: str, exit_code: int) -> Outcome:
    found = _diagnostics(text.splitlines())
    warnings = sum(1 for _, line, _ in found if line.startswith("warning"))
    errors = len(found) - warnings
    details: list[str] = []
    for _, line, where in found:
        detail = f"{line}  ({where})" if where else line
        if detail not in details:
            details.append(detail)
    headline = f"{warnings} warnings, {errors} errors"
    if exit_code == 0 and warnings == 0 and errors == 0:
        return Outcome("ok", headline)
    return Outcome("FAILED", headline, tuple(details) or _tail(text))


#: The line `run_alone` opens each build with, naming the library or the
#: feature set, and the one it ends on.
ALONE_BUILD_RE = re.compile(r"^alone: ([\w/,-]+): cargo ")
ALONE_DONE_RE = re.compile(
    r"^alone: (\d+) librar(?:y|ies), (\d+) feature sets?, (\d+) builds?(?:; |$)"
)


def summarize_alone(text: str, exit_code: int) -> Outcome:
    """Clippy's reading over every build `run_alone` ran: each diagnostic
    counted and listed once, however many builds met it, naming the first
    library or feature set whose build did."""
    lines = text.splitlines()
    done = next((m for line in reversed(lines) if (m := ALONE_DONE_RE.match(line))), None)
    if done is None:
        return Outcome("FAILED", "check.py --alone printed no result", _tail(text))
    starts = [(i, m.group(1)) for i, line in enumerate(lines) if (m := ALONE_BUILD_RE.match(line))]
    seen: dict[tuple[str, str | None], str] = {}
    for i, line, where in _diagnostics(lines):
        library = next((name for at, name in reversed(starts) if at < i), "?")
        seen.setdefault((line, where), library)
    warnings = sum(1 for line, _ in seen if line.startswith("warning"))
    errors = len(seen) - warnings
    headline = (
        f"{done.group(1)} libraries alone, {done.group(2)} feature sets, "
        f"{done.group(3)} builds: {warnings} warnings, {errors} errors"
    )
    if exit_code == 0 and not seen:
        return Outcome("ok", headline)
    details = tuple(
        f"{line}  ({where}; alone: {library})" if where else f"{line}  (alone: {library})"
        for (line, where), library in seen.items()
    )
    return Outcome("FAILED", headline, details or _tail(text))


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


#: `repoint.py`'s two problems that are its meters rather than caps
#: (`meter_problems`); `test_check` pins them against the real script's output.
REPOINT_METER_RE = re.compile(
    r"^!! the (?:live record has grown -?\d+ lines since \S+; repoint"
    r"|decision register has grown -?\d+ bytes since \S+; audit it)$"
)


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


def summarize_module_graph(text: str, exit_code: int) -> Outcome:
    lines = [line.rstrip() for line in text.splitlines()]
    verdict = next((line for line in lines if line.startswith("module-graph: ")), None)
    problems = tuple(line for line in lines if line.startswith("!! "))
    headline = verdict or "module_graph.py printed no result"
    if exit_code == 0:
        return Outcome("ok", headline)
    return Outcome("FAILED", headline, problems or _tail(text))


FILE_LIMITS_METER_RE = re.compile(r"^!! \S+: .* against \d+")


def summarize_file_limits(text: str, exit_code: int) -> Outcome:
    lines = [line.rstrip() for line in text.splitlines()]
    verdict = next((line for line in lines if line.startswith("file-limits: ")), None)
    problems = tuple(line for line in lines if line.startswith("!! "))
    headline = verdict or "file_limits.py printed no result"
    if exit_code == 0:
        return Outcome("ok", headline)
    if exit_code == 1 and problems and all(FILE_LIMITS_METER_RE.match(p) for p in problems):
        return Outcome("meter", headline, problems)
    return Outcome("FAILED", headline, problems or _tail(text))


#: The round's nine commands. `every_check` is what a run without
#: `--affected` runs: these, the doctests narrowed to the libraries holding one.
CHECKS: tuple[Check, ...] = (
    Check("fmt", ("cargo", "fmt", "--check"), ".", summarize_fmt, image=True),
    Check(
        "clippy",
        ("cargo", "clippy", "--workspace", "--all-targets"),
        ".",
        summarize_clippy,
        image=True,
    ),
    Check("alone", ("uv", "run", "check.py", "--alone"), "scripts", summarize_alone, image=True),
    Check(
        "nextest",
        ("cargo", "nextest", "run", "--workspace", "--no-fail-fast"),
        ".",
        summarize_nextest,
        image=True,
    ),
    Check(
        "doctest",
        ("cargo", "test", "--workspace", "--doc", "--no-fail-fast"),
        ".",
        summarize_doctest,
        image=True,
    ),
    Check(
        "graph", ("uv", "run", "module_graph.py"), "scripts", summarize_module_graph, image=True
    ),
    Check("unittest", ("uv", "run", "python", "-m", "unittest"), "scripts", summarize_unittest),
    Check("repoint", ("uv", "run", "repoint.py"), "scripts", summarize_repoint),
    Check("seams", ("uv", "run", "file_limits.py"), "scripts", summarize_file_limits),
)

#: The checks every `--affected` run keeps: cheap, and the ones reading the
#: docs and the tree's shape rather than any one target.
ALWAYS = ("unittest", "repoint", "seams")


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
    #: Whether only dev-dependencies reach it: a test-support library, whose
    #: reads are its dependents' tests' too (`plan_changes`).
    support: bool = False
    #: Whether its manifest marks it the measurement apparatus,
    #: `[package.metadata.pgdt] apparatus = true`: tested by the round, left
    #: out of the release suite (`release.py`, `suite`).
    apparatus: bool = False

    @property
    def has_lib(self) -> bool:
        return any(t.kind == "lib" for t in self.targets)


_MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.M)


def module_files(repo: Path, root: str) -> list[str]:
    """`root` and every file its `mod name;` lines reach, repo-relative, at
    the conventional paths: no member loads a module by `#[path]`
    (`layering.py`). A declaration inside an inline module is not
    followed."""
    out: list[str] = []
    pending = [(root, True)]
    while pending:
        rel, is_mod_root = pending.pop()
        if rel in out:
            continue
        out.append(rel)
        path = repo / rel
        base = path.parent if is_mod_root else path.with_suffix("")
        text = path.read_text(encoding="utf-8", errors="replace")
        for name in _MOD_RE.findall(text):
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


_FENCE = "```"


def code_of(src: str) -> str:
    """Rust source as the compiler and the test suite see it: each comment
    replaced by one space, as the lexer treats it, and every run of whitespace
    outside a literal collapsed to one space; literals verbatim. A doc
    comment's fenced block is a doctest, so each line inside one and each fence
    line is kept verbatim, as is every block doc comment; its prose is dropped.
    Two sources with equal `code_of` differ only in what no build reads."""
    out: list[str] = []
    in_fence = False

    def space() -> None:
        if out and out[-1] != " ":
            out.append(" ")

    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end < 0 else end
            if src.startswith(("///", "//!"), i) and not src.startswith("////", i):
                body = src[i + 3 : end].strip()
                fence = body.startswith(_FENCE)
                if fence or in_fence:
                    out.append(src[i:end])
                if fence:
                    in_fence = not in_fence
            space()
            i = end
        elif src.startswith("/*", i):
            start, depth, i = i, 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            if src.startswith(("/**", "/*!"), start) and not src.startswith(("/***", "/**/"), start):
                out.append(src[start:i])
            space()
        elif (m := _RAW_RE.match(src, i)) and not (i and (src[i - 1].isalnum() or src[i - 1] == "_")):
            close = '"' + m.group(1)
            end = src.find(close, m.end())
            end = n if end < 0 else end + len(close)
            out.append(src[i:end])
            i = end
        elif c == '"':
            m = _PLAIN_STRING_RE.match(src, i)
            end = m.end() if m else n
            out.append(src[i:end])
            i = end
        elif c == "'":
            m = _CHAR_RE.match(src, i)
            end = m.end() if m else i + 1  # a lifetime or a label
            out.append(src[i:end])
            i = end
        elif c.isspace():
            space()
            i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out).strip()


def literal_path(lit: str) -> str | None:
    """The path a literal can name, or `None`. A templated one names the
    directory it fills a name into, so it is cut at its first `{` and then to
    its last `/` (`"../fixtures/{major}"` is `../fixtures/`, and `".{}.tmp"`
    names no directory). A bare `.` names the working directory itself, which is
    a value (a decimal point, a decoded `\\.`) far oftener than a read of
    it, and as a read would make every file in a package one its library's unit
    tests read; whitespace, a backslash or a leading `/` is not a repo path."""
    if "{" in lit:
        lit = lit.split("{", 1)[0]
        lit = lit[: lit.rfind("/") + 1]
    if lit in ("", ".", "./") or any(ch.isspace() for ch in lit) or "\\" in lit or lit.startswith("/"):
        return None
    return lit


def read_prefixes(repo: Path, package_dir: str, source: str) -> set[str]:
    """The repo-relative paths a source's literals can name (`literal_path`),
    taken from the package's directory (a test's working directory, and
    `CARGO_MANIFEST_DIR`) and from the file's own (`include_str!`). `.` is the
    whole repository."""
    text = (repo / source).read_text(encoding="utf-8", errors="replace")
    out: set[str] = set()
    for lit in map(literal_path, string_literals(text)):
        if lit is None:
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
    # The kinds of the edges into each member, `None` being a normal one.
    kinds_into: dict[str, set[str | None]] = {name: set() for name in dirs}
    for p in meta["packages"]:
        if p["id"] not in members:
            continue
        for d in p["dependencies"]:
            if d.get("path"):
                rel = Path(d["path"]).resolve().relative_to(root.resolve()).as_posix()
                if by_dir.get(rel, p["name"]) != p["name"]:
                    kinds_into[by_dir[rel]].add(d.get("kind"))
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
        support = kinds_into[name] == {"dev"}
        apparatus = ((p.get("metadata") or {}).get("pgdt") or {}).get("apparatus") is True
        packages.append(
            Package(name, pdir, frozenset(deps - {name}), tuple(targets), support, apparatus)
        )
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


def comment_only_paths(repo: Path, head_tree: str, tree: str, changed: Sequence[str]) -> set[str]:
    """The changed `.rs` files present in both trees whose `code_of` is equal."""
    out = set()
    for path in changed:
        if not path.endswith(".rs"):
            continue
        try:
            old = _git(repo, "cat-file", "blob", f"{head_tree}:{path}")
            new = _git(repo, "cat-file", "blob", f"{tree}:{path}")
        except subprocess.CalledProcessError:
            continue  # added or deleted
        if code_of(old) == code_of(new):
            out.add(path)
    return out


#: rustdoc's own code-block attributes. An info string made only of these,
#: or naming `rust`, or empty, marks a block `cargo test --doc` compiles; any
#: other word makes it another language's.
RUSTDOC_ATTRS = frozenset(
    {"rust", "ignore", "should_panic", "no_run", "compile_fail", "test_harness", "standalone_crate"}
)
_DOC_LINE_RE = re.compile(r"^\s*(?:///(?!/)|//!)(.*)$")
_DOC_FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")
#: Doc text this file does not read: a block doc comment, and a `doc`
#: attribute, `include_str!` of a README included. Either counts as a doctest.
_UNREAD_DOC_RE = re.compile(r"/\*[*!](?![*/])|#!?\[\s*doc\s*=")


def _rust_info(info: str) -> bool:
    words = [w.removeprefix(".") for w in re.split(r"[\s,{}]+", info) if w]
    return "rust" in words or all(
        w in RUSTDOC_ATTRS or w.startswith(("edition", "ignore-")) for w in words
    )


def has_doctest(src: str) -> bool:
    """Whether rustdoc may compile a code block in `src`'s doc comments: a
    fence whose info string `_rust_info` accepts, or a block indented four
    spaces after a blank line or at a comment's start. Doc text it does not
    read counts as one, so it errs only toward yes, which costs a run."""
    if _UNREAD_DOC_RE.search(src):
        return True
    fence, blank = None, True
    for line in src.splitlines():
        m = _DOC_LINE_RE.match(line)
        if not m:
            fence, blank = None, True
            continue
        body = m.group(1).removeprefix(" ")
        if fence:
            if body.lstrip().startswith(fence):
                fence = None
        elif f := _DOC_FENCE_RE.match(body):
            if _rust_info(f.group(2)):
                return True
            fence = f.group(1)[0] * 3
        elif blank and body.startswith("    ") and body.strip():
            return True
        blank = not body.strip()
    return False


def doctest_packages(repo: Path, packages: Sequence[Package]) -> set[str]:
    """The packages whose library sources hold a doctest (`has_doctest`)."""
    return {
        p.name
        for p in packages
        for t in p.targets
        if t.kind == "lib"
        and any(
            has_doctest((repo / s).read_text(encoding="utf-8", errors="replace"))
            for s in t.sources
        )
    }


def doctest_check(pkgs: Collection[str]) -> Check:
    argv = ("cargo", "test", "--doc", "--no-fail-fast", *(a for n in sorted(pkgs) for a in ("-p", n)))
    return Check("doctest", argv, ".", summarize_doctest, image=True)


#: How `run_alone` builds one library, `-p <name>` appended: its library and
#: binaries as an embedder builds them, then every target, which adds what its
#: dev-dependencies turn on, resolver 2 unifying their features only then.
ALONE_BUILDS: tuple[tuple[str, ...], ...] = (
    ("cargo", "clippy"),
    ("cargo", "clippy", "--all-targets"),
)


@dataclass(frozen=True)
class FeatureBuild:
    """A build the project makes or names to an embedder that no library's own
    build is: `package` under `features`, alone, or over the whole workspace
    where `workspace` -- as the instrument is built, every member's
    `introspect` turned on through `pgdt`'s."""

    package: str
    features: tuple[str, ...]
    default_features: bool = True
    workspace: bool = False

    @property
    def name(self) -> str:
        return f"{self.package}/{','.join(self.features)}"

    @property
    def args(self) -> tuple[str, ...]:
        if self.workspace:
            features = ",".join(f"{self.package}/{f}" for f in self.features)
            return ("--workspace", "--features", features)
        default = () if self.default_features else ("--no-default-features",)
        return ("-p", self.package, *default, "--features", ",".join(self.features))

    def reached(self, linted: Collection[str]) -> bool:
        """Whether a change linting the packages `linted` reaches this set."""
        return bool(linted) if self.workspace else self.package in linted


#: The builds `run_alone` lints beside each library's, each at both
#: `ALONE_BUILDS`. An exclusive choice is built with `--no-default-features`,
#: as `measure.py` builds its legs.
FEATURE_BUILDS: tuple[FeatureBuild, ...] = (
    FeatureBuild("pgdt", ("introspect",), workspace=True),
    FeatureBuild("pgdt", ("jemalloc",), default_features=False),
    FeatureBuild("pgdt", ("system",), default_features=False),
    FeatureBuild("pgdt", ("gzip-1mib-chunks",)),
    FeatureBuild("pgdt-gzip-seek", ("zlib-ng",), default_features=False),
    FeatureBuild("pgdt-gzip-seek", ("zlib",), default_features=False),
    FeatureBuild("datafusion-pgdump", ("http",)),
)

#: `package/feature` -> why no linted build turns it on. `test_check` holds
#: every declared feature to a linted build or an entry here, and every entry
#: to a feature declared and linted by nothing.
UNLINTED_FEATURES: dict[str, str] = {}


def feature_sets(packages: Sequence[Package]) -> list[FeatureBuild]:
    """`FEATURE_BUILDS` whose package is a member: `test_check` holds every one
    of this workspace's to its members."""
    members = {p.name for p in packages}
    return [s for s in FEATURE_BUILDS if s.package in members]


def alone_builds(
    packages: Sequence[Package], names: Collection[str] = ()
) -> list[tuple[str, tuple[str, ...]]]:
    """Each build `run_alone` runs, with the library or feature set it builds:
    of `names`, or of every member holding a library and every feature set. A
    name that is neither is refused."""
    libraries = sorted(p.name for p in packages if p.has_lib)
    sets = {s.name: s for s in feature_sets(packages)}
    if unknown := sorted(set(names) - set(libraries) - set(sets)):
        raise ValueError(f"not a workspace library or feature set: {', '.join(unknown)}")
    chosen = set(names) if names else {*libraries, *sets}
    return [
        *((n, (*b, "-p", n)) for n in libraries if n in chosen for b in ALONE_BUILDS),
        *((n, (*b, *s.args)) for n, s in sets.items() if n in chosen for b in ALONE_BUILDS),
    ]


def run_alone(
    repo: Path, names: Collection[str] = (), load: Callable[[Path], list[Package]] = workspace
) -> int:
    """Lint each library alone (`ALONE_BUILDS`), one cargo run each, so its
    features are its own rather than what the workspace unifies on, and each
    feature set (`FEATURE_BUILDS`) the same way; every build runs though one
    fails. The output is the cargo runs', each opened by a line naming its
    library or set, then a count: `summarize_alone` reads it."""
    packages = load(repo)
    builds = alone_builds(packages, names)
    failed: list[str] = []
    for name, argv in builds:
        print(f"alone: {name}: {shlex.join(argv)}", flush=True)
        code = subprocess.run(
            argv, cwd=repo, stdin=subprocess.DEVNULL, stderr=subprocess.STDOUT
        ).returncode
        sys.stdout.flush()
        if code != 0:
            failed.append(f"{shlex.join(argv)} exited {code}")
    set_names = {s.name for s in feature_sets(packages)}
    built = {name for name, _ in builds}
    libraries, sets = len(built - set_names), len(built & set_names)
    tail = f"; {len(failed)} failed: {', '.join(failed)}" if failed else ""
    print(
        f"alone: {libraries} libraries, {sets} feature sets, {len(builds)} builds{tail}",
        flush=True,
    )
    return 1 if failed else 0


def alone_check(names: Collection[str]) -> Check:
    argv = ("uv", "run", "check.py", "--alone", *sorted(names))
    return Check("alone", argv, "scripts", summarize_alone, image=True)


def every_check(repo: Path, load: Callable[[Path], list[Package]] = workspace) -> tuple[Check, ...]:
    """`CHECKS`, the doctests run for the libraries holding one, or not at all."""
    docs = doctest_packages(repo, load(repo))
    return tuple(
        doctest_check(docs) if c.name == "doctest" else c
        for c in CHECKS
        if c.name != "doctest" or docs
    )


def plan_affected(
    repo: Path,
    tree: str,
    head_tree: str | None,
    load: Callable[[Path], list[Package]] = workspace,
) -> Plan:
    if head_tree is None:
        return Plan(
            every_check(repo, load),
            "affected: no commit to compare with, so every check runs",
        )
    changed = changed_paths(repo, head_tree, tree)
    return plan_changes(repo, changed, load, comment_only_paths(repo, head_tree, tree, changed))


def plan_changes(
    repo: Path,
    changed: Sequence[str],
    load: Callable[[Path], list[Package]] = workspace,
    comment_only: Collection[str] = (),
) -> Plan:
    always = tuple(c for c in CHECKS if c.name in ALWAYS)
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
    whole, roots, local, linted = None, set(), set(), set()
    reader_ids: set[tuple[str, str]] = set()
    for path in changed:
        found = {
            (pkg, t.binary_id)
            for (pkg, t), pre in prefixes.items()
            if any(reads(x, path) for x in pre)
        }
        # A test-support library reads for each test depending on it: any of
        # them may call what reads.
        for pkg in {pkg for pkg, bid in found if bid == pkg and by_name[pkg].support}:
            found |= {(q.name, t.binary_id) for q in packages if pkg in q.deps for t in q.targets}
        reader_ids |= found
        if UNTESTED_RE.search(path):
            continue
        home = next((p for p in packages if path.startswith(p.dir + "/")), None)
        if home is not None:
            rest = path[len(home.dir) + 1 :]
            if path in comment_only:
                linted.add(home.name)
            else:
                (local if rest.startswith(PACKAGE_LOCAL) else roots).add(home.name)
        elif not found and whole is None:
            whole = path
    if whole is not None:
        return Plan(
            every_check(repo, load),
            f"affected: {whole} is in no crate and read by no test, so every check runs",
        )
    closure = set(roots)
    grew = True
    while grew:
        more = {p.name for p in packages if p.deps & closure} - closure
        closure |= more
        grew = bool(more)
    whole_pkgs = sorted(closure | local)
    binaries = sorted((pkg, bid) for pkg, bid in reader_ids if pkg not in whole_pkgs)
    doc_libs = {n for n in whole_pkgs if by_name[n].has_lib} | {
        pkg for pkg, bid in binaries if bid == pkg
    }
    doc_pkgs = sorted(doc_libs & doctest_packages(repo, [by_name[n] for n in doc_libs]))
    lint_only = sorted(linted - set(whole_pkgs))
    checks: list[Check] = []
    p_args = tuple(a for n in sorted({*whole_pkgs, *lint_only}) for a in ("-p", n))
    if p_args:
        fmt = ("cargo", "fmt", "--check", *p_args)
        checks.append(Check("fmt", fmt, ".", summarize_fmt, image=True))
        clippy = ("cargo", "clippy", "--all-targets", *p_args)
        checks.append(Check("clippy", clippy, ".", summarize_clippy, image=True))
        linted_pkgs = {*whole_pkgs, *lint_only}
        libraries = [n for n in linted_pkgs if by_name[n].has_lib]
        sets = [s.name for s in feature_sets(packages) if s.reached(linted_pkgs)]
        if libraries or sets:
            checks.append(alone_check([*libraries, *sets]))
    if whole_pkgs or binaries:
        run_pkgs = sorted(set(whole_pkgs) | {pkg for pkg, _ in binaries})
        argv = ["cargo", "nextest", "run", "--no-fail-fast"]
        argv += [a for n in run_pkgs for a in ("-p", n)]
        if binaries:
            terms = [f"package(={n})" for n in whole_pkgs] + [f"binary_id(={b})" for _, b in binaries]
            argv += ["-E", " | ".join(terms)]
        checks.append(Check("nextest", tuple(argv), ".", summarize_nextest, image=True))
    if doc_pkgs:
        checks.append(doctest_check(doc_pkgs))
    if whole_pkgs:
        checks.extend(c for c in CHECKS if c.name == "graph")
    parts = [f"{_paths(len(changed))} changed since HEAD"]
    if whole_pkgs:
        parts.append(f"packages {', '.join(whole_pkgs)}")
    if lint_only:
        parts.append(f"linted only, their changes being comments, {', '.join(lint_only)}")
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
    meters = {c["name"] for c in record["checks"] if c["status"] == "meter"}
    if "repoint" in meters:
        parts.append("repoint's meter is red, so a repoint is due")
    if "seams" in meters:
        parts.append("the seams meter is red, so a seams round is due")
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
        argv, cwd = check.command()
        t0 = time.monotonic()
        with open(repo / log, "w", encoding="utf-8") as sink:
            try:
                code = subprocess.run(
                    argv,
                    cwd=repo / cwd,
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
            "argv": list(argv),
            "cwd": cwd,
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


def runs_of(repo: Path, tree: str) -> list[dict]:
    """Every recorded run over this tree, of any list of checks, passed or not,
    oldest first."""
    root = repo / RUNS
    if not root.is_dir():
        return []
    found = []
    for run_dir in root.iterdir():
        try:
            record = json.loads((run_dir / "record.json").read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if record.get("tree") == tree:
            found.append(record)
    return sorted(found, key=lambda r: r.get("epoch", 0))


def _ran(record: dict) -> list:
    return [[c["name"], c["argv"], c["cwd"]] for c in record.get("checks", [])]


def verify(
    repo: Path,
    checks: Sequence[Check] = CHECKS,
    out: TextIO = sys.stdout,
    note: str | None = None,
    tree: str | None = None,
    also: Sequence[Sequence[Check]] = (),
) -> int:
    """Reuse the newest run of `checks` over this tree -- or of a list in
    `also`, the whole list for an `--affected` verify -- if it passed and no
    later run of the tree, of any list, failed; else `run`."""
    tree = tree or tree_stamp(repo)
    wanted = [[c.key() for c in cs] for cs in (checks, *also)]
    runs = runs_of(repo, tree)
    ours = [i for i, r in enumerate(runs) if _ran(r) in wanted]
    if not ours:
        print("check: no run of this tree; running the checks", file=out, flush=True)
        return run(repo, checks, out, note, tree)
    since = runs[ours[-1] :]
    failed = next((r for r in reversed(since) if not r.get("passed")), None)
    if failed is not None:
        print(f"check: {failed['dir']} did not pass; running the checks", file=out, flush=True)
        return run(repo, checks, out, note, tree)
    record = since[0]
    print(
        f"check: reusing {record['dir']} ({record['started']}), the newest such run of this tree",
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
        help="reuse this tree's passing run unless a later run of it failed; run the checks otherwise",
    )
    parser.add_argument(
        "--affected",
        action="store_true",
        help="run only the cargo checks the change since HEAD can reach",
    )
    parser.add_argument(
        "--alone",
        nargs="*",
        metavar="NAME",
        help="lint each library or feature set named, or every one, alone: the alone check",
    )
    args = parser.parse_args(argv)
    if args.alone is not None:
        if args.affected or args.verify:
            parser.error("--alone is one check, run by the others")
        return run_alone(REPO, args.alone)
    every = every_check(REPO)
    if not args.affected:
        return (verify if args.verify else run)(REPO, every)
    tree = tree_stamp(REPO)
    plan = plan_affected(REPO, tree, head_tree(REPO)[1])
    if args.verify:
        return verify(REPO, plan.checks, note=plan.note, tree=tree, also=(every,))
    return run(REPO, plan.checks, note=plan.note, tree=tree)


if __name__ == "__main__":
    raise SystemExit(main())
