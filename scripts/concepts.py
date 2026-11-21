#!/usr/bin/env python3
"""The concept map held to the tree: every production file claimed once.

`docs/design/concepts.md` names each concept the code holds and the modules
holding it, crate by crate and in the scripts (`docs/process.md`, "Seams").
This holds the map to the workspace and to `scripts/`:

* a **claim** is a bullet `- **<crate>** — …` under a concept's `## ` heading,
  its wrapped lines included; every backticked path in it ending `.rs` or `/`
  is claimed, relative to `crates/<crate>/src/`. A directory claims every
  file under it and the `<dir>.rs` beside it. A bullet `- **scripts** — …`
  claims each path ending `.py` or `/` in it relative to `scripts/`, a
  directory every file under it;
* every **production file** -- a `.rs` under a member's `src/`, or a `.py`
  under `scripts/`, that is not a test file by `file_limits.py`'s reading --
  is claimed by exactly one concept, a root (`lib.rs`, `main.rs`, a package's
  `__init__.py`) excepted unless claimed;
* every claimed path names a production file or directory of that crate or of
  the scripts, and every crate named is a workspace member;
* every concept is headed once and claims something.

What a concept is, and whether a file is in the right one, is the review's;
this checks only that the map and the tree agree. Prose outside a claim may
name any path, including another crate's.

Usage:

    cd scripts && uv run concepts.py
    cd scripts && uv run python -m unittest test_concepts
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import TextIO

import file_limits

REPO = Path(__file__).resolve().parent.parent
MAP = "docs/design/concepts.md"

HEADING_RE = re.compile(r"^## (.+?)\s*$")
CLAIM_RE = re.compile(r"^- \*\*([\w-]+)\*\* — ")
PATH_RE = re.compile(r"`([\w./-]+(?:\.rs|\.py|/))`")
ROOTS = ("lib.rs", "main.rs")
#: The claimant naming `scripts/`, beside the workspace's members.
SCRIPTS = "scripts"
PACKAGE_ROOT = "__init__.py"
NOT_IN_ONE_PLACE = "**Not in one place.**"


def claims(text: str) -> tuple[dict[str, list[tuple[str, str, int]]], list[str], list[str]]:
    """Each concept to its claims `(crate, path, line)`, the concepts with a
    "Not in one place" paragraph, and the problems found reading the map."""
    found: dict[str, list[tuple[str, str, int]]] = {}
    scattered: list[str] = []
    problems: list[str] = []
    concept: str | None = None
    crate: str | None = None
    for n, line in enumerate(text.splitlines(), 1):
        if m := HEADING_RE.match(line):
            concept, crate = m.group(1), None
            if concept in found:
                problems.append(f"{MAP}:{n}: concept {concept!r} is headed twice")
            found[concept] = []
            continue
        if line.startswith("#"):
            concept, crate = None, None
            continue
        if m := CLAIM_RE.match(line):
            if concept is None:
                problems.append(f"{MAP}:{n}: a claim outside any concept")
                crate = None
                continue
            crate = m.group(1)
        elif not (crate and line.startswith("  ")):
            crate = None
        if concept is not None and line.startswith(NOT_IN_ONE_PLACE):
            scattered.append(concept)
        if crate is not None and concept is not None:
            for p in PATH_RE.findall(line):
                found[concept].append((crate, p, n))
    return found, scattered, problems


def production_files(repo: Path) -> dict[str, set[str]]:
    """Each member's directory name to its production files, relative to
    its `src/`, and `SCRIPTS` to the scripts' own, relative to `scripts/`."""
    out: dict[str, set[str]] = {m.name: set() for m in file_limits.members(repo)}
    for r in file_limits.readings(repo):
        parts = Path(r.path).parts
        if r.is_test or len(parts) < 4 or parts[0] != "crates" or parts[2] != "src":
            continue
        out.setdefault(parts[1], set()).add("/".join(parts[3:]))
    out[SCRIPTS] = {
        r.path.removeprefix(f"{SCRIPTS}/")
        for r in file_limits.script_readings(repo)
        if not r.is_test
    }
    return out


def is_root(claimant: str, f: str) -> bool:
    """A crate root, or a package's `__init__.py`: claimed by none unless
    it holds more than its doc, its imports and its re-exports."""
    if claimant == SCRIPTS:
        return Path(f).name == PACKAGE_ROOT
    return f in ROOTS


def located(claimant: str, f: str) -> str:
    """A claimed file's repo-relative path."""
    return f"{SCRIPTS}/{f}" if claimant == SCRIPTS else f"crates/{claimant}/src/{f}"


def resolve(claim: str, files: set[str]) -> set[str]:
    """The files a claimed path names: itself, or a directory's contents and
    the `<dir>.rs` beside it."""
    if claim.endswith("/"):
        stem = claim.rstrip("/")
        return {f for f in files if f.startswith(claim) or f == f"{stem}.rs"}
    return {claim} & files


def check(repo: Path = REPO, out: TextIO = sys.stdout) -> int:
    path = repo / MAP
    if not path.exists():
        print(f"error: {MAP} is missing", file=out)
        return 1
    found, scattered, problems = claims(path.read_text(encoding="utf-8"))
    files = production_files(repo)
    owner: dict[tuple[str, str], str] = {}
    for concept, held in found.items():
        if not held:
            problems.append(f"{MAP}: concept {concept!r} claims nothing")
        for crate, claim, n in held:
            if crate not in files:
                problems.append(f"{MAP}:{n}: {crate} is not a workspace member")
                continue
            named = resolve(claim, files[crate])
            if not named:
                problems.append(
                    f"{MAP}:{n}: {crate}'s `{claim}` names no production file"
                )
            for f in sorted(named):
                if (crate, f) in owner:
                    problems.append(
                        f"{MAP}:{n}: {located(crate, f)} is claimed by "
                        f"{owner[crate, f]!r} and {concept!r}"
                    )
                else:
                    owner[crate, f] = concept
    for crate, held in sorted(files.items()):
        for f in sorted(held):
            if (crate, f) not in owner and not is_root(crate, f):
                problems.append(f"{located(crate, f)} is claimed by no concept in {MAP}")
    if problems:
        for p in problems:
            print(f"error: {p}", file=out)
        print(f"concepts: {len(problems)} problem(s)", file=out)
        return 1
    spanning = sum(
        1 for held in found.values() if len({c for c, _, _ in held} - {SCRIPTS}) > 1
    )
    reaching = sum(1 for held in found.values() if any(c == SCRIPTS for c, _, _ in held))
    scripts = sum(1 for c, _ in owner if c == SCRIPTS)
    print(
        f"concepts: {len(found)} concepts claim {len(owner)} files once each, "
        f"{scripts} of them scripts; {spanning} span crates, {reaching} reach the "
        f"scripts, {len(scattered)} not in one place",
        file=out,
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--repo", type=Path, default=REPO, help="repository root (tests use this)")
    args = parser.parse_args(argv)
    return check(args.repo)


if __name__ == "__main__":
    raise SystemExit(main())
