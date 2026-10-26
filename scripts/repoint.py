#!/usr/bin/env python3
"""The repoint check: the record's caps, and how far the live record has grown
since it was last read against the code.

`docs/process.md`, "Repointing" is the rule. The keystone that struck P19 found
about 125 claims the code no longer bore out, because nothing between one
keystone and the next reads the standing record against the code, and nothing
capped what each landing added to it: the subject-filed design doc grew by some
five hundred lines a day and the dated entries ran ten times their stated
length. Both halves are mechanical, so both are here:

* **The caps.** Each register entry at its character cap, no line of one that
  could wrap, the index and the topic files naming the same entries; a
  dated entry, from the day the cap was adopted, at the length
  `docs/status/history/README.md` states;
  `CLAUDE.md`, `docs/process.md` and each skill at the sizes that keep
  orientation cheap. Adding under a full one means striking; a session never
  raises a cap to fit what it is landing, and only the maintainer moves one.
* **No narration where a conclusion belongs.** No measured quantity in the
  register or in `STATUS.md`'s capability table (a number is stated once, in
  `measurements.md`); no phase, ledger or date provenance in a source comment
  (a comment states the contract and cites `D<k>`). Numbers in comments are not
  checked: help text states a default and a test comment does arithmetic over
  its fixture, both legitimately, and telling those from a quoted figure is the
  reader's job, not a regex's.
* **The meter.** `STATUS.md` carries `<!-- repointed: <sha> -->`, the commit
  the record was last read against. The net growth since then of the *live*
  record -- every standing `.md` outside `history/` and the phase docs, the
  skills, and the comment lines of every `.rs` -- is reported, and past the
  budget the check is red. Red is the trigger for a repoint; the phase docs are
  centering and history is deleted at a keystone, so neither is counted.

Usage:

    cd scripts
    uv run repoint.py [--stamp <sha>] [--budget <lines>]
    uv run python -m unittest test_repoint
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence

REPO = Path(__file__).resolve().parent.parent

#: The register: `docs/process.md`, "The decision register". The index is
#: `decisions.md`, one `- **D<k>** <title>` line an entry; the bodies are the
#: topic files under `REGISTER_DIR`. An entry is capped in characters, the
#: unit a writer cannot absorb by reformatting (a line cap was met six times
#: by lengthening lines, never by striking), and a line that could wrap under
#: `LINE_COLUMNS` is named. The register whole has no cap: a full one strikes
#: a live decision to admit the next. Its growth since the repoint stamp is a
#: meter (`REGISTER_GROWTH_BYTES`), red meaning an audit of every entry
#: against the code, which is what pruned it on 2026-10-25.
REGISTER_DIR = "docs/design/decisions"
ENTRY_CHARS = 850
LINE_COLUMNS = 120
REGISTER_GROWTH_BYTES = 16 * 1024
#: The one statement of this is `docs/status/history/README.md`; this is that
#: number, held only for entries dated after the day the cap was adopted. Raised
#: from 120 on 2026-09-18 (`docs/status/history/2026-09-18.md`), to 250 on
#: 2026-09-22 and to 350 on 2026-09-26, by the maintainer, a day's file being
#: open to its own sessions to condense.
HISTORY_LINES = 350
HISTORY_CAP_FROM = "2026-09-13"
#: Orientation is what the read-triggers pull into context, so these are the
#: caps that decide what a session costs before it has done anything.
#: `docs/process.md`'s raised from 525, and the skills' from 200, on 2026-10-12
#: by the maintainer, for the seam register.
ORIENTATION_CAPS = {
    "CLAUDE.md": 150,
    "docs/process.md": 550,
}
SKILL_LINES = 225
#: Net lines the live record may grow between repoints before this goes red.
#: P19 grew it by about 1,900 a day, so this is two such days; under the caps
#: and the citation rule it should be many more. Tune it, never to fit.
GROWTH_BUDGET = 3000

STAMP_RE = re.compile(r"<!-- repointed: ([0-9a-f]{7,40}) -->")
ENTRY_RE = re.compile(r"^### (D\d+)\b ?(.*)$")
INDEX_RE = re.compile(r"^- \*\*(D\d+)\*\* (.*)$")
WATERMARK_RE = re.compile(r"<!-- decision-watermark: D(\d+) -->")
HISTORY_NAME_RE = re.compile(r"^(\d{4}-\d{2}-\d{2})\.md$")
#: A measured quantity: a multiplier, a byte or time quantity, a percentage.
MEASURED_RE = re.compile(
    r"\d+(?:\.\d+)?\s?×|\b\d+(?:\.\d+)?\s?(?:[KMG]i?B|ms|µs|s)\b|\b\d+(?:\.\d+)?\s?%"
)
#: Provenance: a slice, a ledger row, a date. `D<k>`, `KD<k>`, `I<n>` and
#: `RT<n>` are handles, not provenance, and are what a comment should carry.
PROVENANCE_RE = re.compile(r"\bP\d+\.\d+\b|\bM\d{1,3}\b|\b2026-\d\d-\d\d\b")
COMMENT_RE = re.compile(r"^\s*//")
CODE_ROOTS = ("crates",)


@dataclass(frozen=True)
class Growth:
    docs: int
    comments: int

    @property
    def total(self) -> int:
        return self.docs + self.comments


def _lines(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines()


def register_files(repo: Path) -> list[Path]:
    """The index and every topic file, the paths the register's size is read over."""
    index = repo / "docs" / "design" / "decisions.md"
    topics = sorted((repo / REGISTER_DIR).glob("*.md")) if (repo / REGISTER_DIR).exists() else []
    return [index, *topics]


def register_problems(repo: Path) -> list[str]:
    index, *topics = register_files(repo)
    if not index.exists():
        return [f"{index.relative_to(repo)}: missing"]
    problems = []
    text = index.read_text(encoding="utf-8")
    indexed: dict[str, str] = {}
    for line in text.splitlines():
        m = INDEX_RE.match(line)
        if m and m.group(1) in indexed:
            problems.append(f"decisions.md indexes {m.group(1)} twice")
        elif m:
            indexed[m.group(1)] = m.group(2).strip()
    watermark = WATERMARK_RE.search(text)
    entries: dict[str, tuple[str, str]] = {}
    for path in topics:
        rel = path.relative_to(repo)
        entry, chars = None, 0

        def close() -> None:
            if entry and chars > ENTRY_CHARS:
                problems.append(f"{rel} {entry} runs {chars} characters against {ENTRY_CHARS}")

        for n, line in enumerate(_lines(path) + ["## end"], 1):
            m = ENTRY_RE.match(line)
            if m or line.startswith("## "):
                close()
                entry, chars = (m.group(1) if m else None), 0
                if m and m.group(1) in entries:
                    problems.append(f"{m.group(1)} is held by {entries[m.group(1)][0]} and {rel}")
                elif m:
                    entries[m.group(1)] = (str(rel), m.group(2).strip())
            elif line.strip():
                chars += len(line.strip())
            if len(line) > LINE_COLUMNS and " " in line[LINE_COLUMNS:] and not line.startswith("#"):
                problems.append(f"{rel}:{n} runs past {LINE_COLUMNS} columns and could wrap")
            if MEASURED_RE.search(line):
                problems.append(f"{rel}:{n} quotes a measured quantity: {line.strip()}")
    for d, (rel, title) in sorted(entries.items(), key=lambda kv: int(kv[0][1:])):
        if d not in indexed:
            problems.append(f"{rel} holds {d}, which decisions.md does not index")
        elif indexed[d] != title:
            problems.append(f"{d} is indexed as {indexed[d]!r} but headed {title!r} in {rel}")
    for d in sorted(set(indexed) - set(entries), key=lambda d: int(d[1:])):
        problems.append(f"decisions.md indexes {d}, which no topic file holds")
    if watermark is None:
        problems.append("decisions.md carries no `<!-- decision-watermark: D<k> -->`")
    elif entries and int(watermark.group(1)) < max(int(d[1:]) for d in entries):
        problems.append(f"the decision watermark D{watermark.group(1)} is below an entry")
    return problems


def register_bytes(repo: Path, rev: str | None = None) -> int:
    """The register's size in bytes, in the working tree or at `rev`."""
    if rev is None:
        return sum(p.stat().st_size for p in register_files(repo) if p.exists())
    listing = _git(repo, "ls-tree", "-r", "-l", rev, "--", "docs/design/decisions.md", REGISTER_DIR)
    return sum(int(row.split()[3]) for row in listing.splitlines() if row.strip())


def history_problems(repo: Path) -> list[str]:
    root = repo / "docs" / "status" / "history"
    problems = []
    for path in sorted(root.glob("*.md")) if root.exists() else []:
        m = HISTORY_NAME_RE.match(path.name)
        if not m or m.group(1) <= HISTORY_CAP_FROM:
            continue
        n = len(_lines(path))
        if n > HISTORY_LINES:
            problems.append(f"history/{path.name} is {n} lines against {HISTORY_LINES}")
    return problems


def orientation_problems(repo: Path) -> list[str]:
    problems = []
    for rel, cap in ORIENTATION_CAPS.items():
        path = repo / rel
        if path.exists() and len(_lines(path)) > cap:
            problems.append(f"{rel} is {len(_lines(path))} lines against {cap}")
    for path in sorted((repo / ".claude" / "skills").glob("*/SKILL.md")):
        n = len(_lines(path))
        if n > SKILL_LINES:
            problems.append(
                f"{path.relative_to(repo)} is {n} lines against {SKILL_LINES}"
            )
    return problems


def status_problems(repo: Path) -> list[str]:
    path = repo / "docs" / "status" / "STATUS.md"
    if not path.exists():
        return [f"{path.relative_to(repo)}: missing"]
    problems, in_table = [], False
    for n, line in enumerate(_lines(path), 1):
        if line.startswith("## "):
            in_table = line.strip() == "## What exists"
        elif in_table and line.startswith("|") and MEASURED_RE.search(line):
            problems.append(f"STATUS.md:{n} quotes a measured quantity in \"What exists\"")
    return problems


def _source_files(repo: Path) -> Iterable[Path]:
    for root in CODE_ROOTS:
        yield from sorted((repo / root).rglob("*.rs"))


def comment_problems(repo: Path) -> list[str]:
    problems = []
    for path in _source_files(repo):
        for n, line in enumerate(_lines(path), 1):
            if COMMENT_RE.match(line) and PROVENANCE_RE.search(line):
                problems.append(
                    f"{path.relative_to(repo)}:{n} carries provenance: {line.strip()}"
                )
    return problems


def read_stamp(repo: Path) -> str | None:
    path = repo / "docs" / "status" / "STATUS.md"
    if not path.exists():
        return None
    found = STAMP_RE.findall(path.read_text(encoding="utf-8"))
    return found[0] if len(found) == 1 else None


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout


def is_live_doc(path: str) -> bool:
    if not path.endswith(".md") or path.startswith("vendor/"):
        return False
    if "/history/" in path or re.match(r"docs/design/roadmap-P\d+-", path):
        return False
    return (
        path.startswith("docs/")
        or path.startswith(".claude/skills/")
        or path in ("CLAUDE.md", "README.md", "CONTRIBUTING.md")
    )


def is_source(path: str, roots: Sequence[str] = CODE_ROOTS) -> bool:
    return path.endswith(".rs") and path.startswith(tuple(f"{root}/" for root in roots))


def members_at(repo: Path, rev: str) -> tuple[str, ...]:
    """The workspace members' roots as `rev`'s manifest lists them: a member
    since moved under `CODE_ROOTS` was a source where it stood, so a file
    whose move git does not pair with its old path is subtracted there."""
    try:
        manifest = tomllib.loads(_git(repo, "show", f"{rev}:Cargo.toml"))
    except (subprocess.CalledProcessError, tomllib.TOMLDecodeError):
        return ()
    members = manifest.get("workspace", {}).get("members", [])
    return tuple(m.removesuffix("/*") for m in members)


def growth_since(repo: Path, stamp: str) -> Growth:
    """Net lines added to the live record between `stamp` and the working tree,
    untracked files included, so a round is measured before it is committed."""
    docs = 0
    for row in _git(repo, "diff", "--numstat", stamp, "--", ".").splitlines():
        added, deleted, path = row.split("\t", 2)
        if added != "-" and is_live_doc(path):
            docs += int(added) - int(deleted)
    comments = 0
    # `is_source` per file, as the untracked pass below does: a `.rs` outside
    # `CODE_ROOTS` is not this project's record, and a re-vendor of the frozen
    # copy would otherwise spend the budget on comments no repoint may rake.
    roots = (*CODE_ROOTS, *members_at(repo, stamp))
    old, counting = "", False
    for line in _git(repo, "diff", "-U0", stamp, "--", "*.rs").splitlines():
        if line.startswith("--- "):
            old = line[6:] if line.startswith("--- a/") else ""
            continue
        if line.startswith("+++ "):
            new = line[6:] if line.startswith("+++ b/") else ""
            counting = is_source(new or old, roots)
            continue
        if counting and line[:1] in "+-" and COMMENT_RE.match(line[1:]):
            comments += 1 if line[0] == "+" else -1
    # Read only what is record: an untracked file may be binary (a fixture).
    for path in _git(repo, "ls-files", "--others", "--exclude-standard").splitlines():
        if is_live_doc(path):
            docs += len(_lines(repo / path))
        elif is_source(path):
            comments += sum(1 for line in _lines(repo / path) if COMMENT_RE.match(line))
    return Growth(docs, comments)


def meter_problems(repo: Path, stamp: str | None, budget: int, out) -> list[str]:
    if stamp is None:
        return ["STATUS.md carries no single `<!-- repointed: <sha> -->` stamp"]
    try:
        _git(repo, "merge-base", "--is-ancestor", stamp, "HEAD")
    except subprocess.CalledProcessError:
        return [f"the repoint stamp {stamp} is not an ancestor of HEAD"]
    g = growth_since(repo, stamp)
    register = register_bytes(repo) - register_bytes(repo, stamp)
    print(
        f"since {stamp}: live docs {g.docs:+d}, source comments {g.comments:+d}, "
        f"net {g.total:+d} of a {budget}-line budget; the decision register "
        f"{register:+d} of a {REGISTER_GROWTH_BYTES}-byte budget",
        file=out,
    )
    problems = []
    if g.total > budget:
        problems.append(f"the live record has grown {g.total} lines since {stamp}; repoint")
    if register > REGISTER_GROWTH_BYTES:
        problems.append(f"the decision register has grown {register} bytes since {stamp}; audit it")
    return problems


def check(
    repo: Path = REPO, stamp: str | None = None, budget: int = GROWTH_BUDGET, out=sys.stdout
) -> int:
    problems = (
        register_problems(repo)
        + history_problems(repo)
        + orientation_problems(repo)
        + status_problems(repo)
        + comment_problems(repo)
        + meter_problems(repo, stamp or read_stamp(repo), budget, out)
    )
    for p in problems:
        print(f"!! {p}", file=out)
    print("repoint: " + ("red" if problems else "green"), file=out)
    return 1 if problems else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--repo", type=Path, default=REPO, help="repository root")
    parser.add_argument("--stamp", help="measure growth from this commit instead of the stamp")
    parser.add_argument("--budget", type=int, default=GROWTH_BUDGET)
    args = parser.parse_args(argv)
    return check(args.repo.resolve(), args.stamp, args.budget)


if __name__ == "__main__":
    sys.exit(main())
