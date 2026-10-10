#!/usr/bin/env python3
"""Every Rust file against the size limits; red means a seams round.

The rule is `docs/process.md`, "Seams"; this is its check, and the one place
the numbers are stated. A file is read three ways:

* **code lines** -- lines of its production code holding something that is
  neither a comment nor whitespace, every `#[cfg(test)] mod` block excluded;
* **functions** -- `fn` items in that same production code, methods included,
  closures and `fn` pointer types not;
* **total lines** -- the file as it is, tests and comments included.

The first two carry a soft limit, past which a split is due when someone is
next in the file, and a hard one, past which it is due now; the third carries
a hard limit alone, the one that says a file has become arduous by sheer
length. A **test file** -- one under a `tests/` directory, or one a
`#[cfg(test)] mod name;` in its parent declares -- is measured against the
total-lines limit only, having no production code.

**An exemption names a file allowed past a hard limit and says why**
(`EXEMPT`). An exemption for a file under every hard limit, or for a file that
is gone, is stale and fails the check outright: the table is read as a list of
live debts, and a settled one has to leave it.

**Output.** One line a file past a limit -- `!! ` past a hard one, `.. ` past
a soft one, `== ` exempt -- then a verdict line. The exit status is 0 when no
file is past a hard limit, 1 when one is (the meter `check.py` reports without
failing), and 2 for a stale exemption. `--all` prints every file's reading.

Comments and string literals are masked before anything is counted, so a `fn`
or a brace inside either is not seen (`masked`); the masking keeps every
newline, so line numbers survive it. It is a scanner, not a parser: it knows
line and block comments, ordinary, raw and byte strings, char literals and
lifetimes, and nothing else, which is all the counting needs.

Usage:

    cd scripts && uv run file_limits.py [--all]
    cd scripts && uv run python -m unittest test_file_limits
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

REPO = Path(__file__).resolve().parent.parent

#: The limits. The maintainer's first pass, 2026-10-10, chosen to bite on the
#: files that had become grab-bags and nowhere else; tightening is an edit here
#: and in `test_file_limits`, which pins each one.
CODE_SOFT = 1000
CODE_HARD = 2500
FNS_SOFT = 40
FNS_HARD = 100
TOTAL_HARD = 7500

#: Files allowed past a hard limit, repo-relative, each with the reason a cut
#: would make the file worse rather than smaller. Read as live debts: one that
#: no longer holds fails the check.
EXEMPT: dict[str, str] = {
    "crates/pgdt-codec/src/datetime_in.rs": (
        "a transliteration of upstream datetime.c, sectioned as the original is; "
        "a cut would un-mirror the port, and P33 reconsiders it"
    ),
}

STRING_START_RE = re.compile(r'b?r(#*)"|b?"')
FN_RE = re.compile(r"(?<![\w'])fn\s+[A-Za-z_]\w*")
CFG_TEST = "#[cfg(test)]"
MOD_RE = re.compile(r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*([{;])")


def _blank(span: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in span)


def _fill(span: str) -> str:
    return "".join("\n" if ch == "\n" else "x" for ch in span)


def masked(text: str) -> str:
    """`text` with every comment blanked and every string or char literal's
    contents replaced by `x`, newlines kept."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        two = text[i : i + 2]
        prev_ident = i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_")
        if two == "//":
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(_blank(text[i:j]))
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text[j : j + 2] == "/*":
                    depth, j = depth + 1, j + 2
                elif text[j : j + 2] == "*/":
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(_blank(text[i:j]))
            i = j
        elif (m := STRING_START_RE.match(text, i)) and not prev_ident:
            start = m.end()
            if m.group(1) is not None:
                close = '"' + m.group(1)
                at = text.find(close, start)
                end = n if at < 0 else at
            else:
                close = '"'
                end = start
                while end < n and text[end] != '"':
                    end += 2 if text[end] == "\\" else 1
            j = min(end + len(close), n)
            out.append(m.group(0) + _fill(text[start:end]) + (close if end < n else ""))
            i = j
        elif c == "'" and (
            not prev_ident
            or (text[i - 1] == "b" and not (i > 1 and (text[i - 2].isalnum() or text[i - 2] == "_")))
        ):
            if text[i + 1 : i + 2] == "\\":
                j = text.find("'", i + 2)
                j = n if j < 0 else j + 1
            elif text[i + 2 : i + 3] == "'":
                j = i + 3
            else:  # a lifetime
                out.append(c)
                i += 1
                continue
            out.append("'" + _fill(text[i + 1 : j - 1]) + "'")
            i = j
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _close_brace(text: str, open_at: int) -> int:
    depth = 0
    for k in range(open_at, len(text)):
        if text[k] == "{":
            depth += 1
        elif text[k] == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(text) - 1


def test_blocks(masked_text: str) -> tuple[list[tuple[int, int]], list[str]]:
    """The line ranges (0-based, inclusive) of every `#[cfg(test)] mod x {}`
    block, and the names every `#[cfg(test)] mod x;` declares."""
    ranges: list[tuple[int, int]] = []
    declared: list[str] = []
    at = 0
    while (at := masked_text.find(CFG_TEST, at)) >= 0:
        m = MOD_RE.match(masked_text, at + len(CFG_TEST))
        if m is None:
            at += len(CFG_TEST)
            continue
        if m.group(2) == ";":
            declared.append(m.group(1))
            at = m.end()
            continue
        end = _close_brace(masked_text, m.end() - 1)
        ranges.append((masked_text.count("\n", 0, at), masked_text.count("\n", 0, end)))
        at = end + 1
    return ranges, declared


def declared_test_files(path: Path, names: list[str]) -> list[Path]:
    """Where `#[cfg(test)] mod name;` in `path` resolves to."""
    base = path.parent if path.name in ("lib.rs", "main.rs", "mod.rs") else path.with_suffix("")
    return [base / f"{name}.rs" for name in names]


@dataclass(frozen=True)
class Reading:
    path: str  # repo-relative
    total: int
    code: int
    fns: int
    test_lines: int
    is_test: bool

    def over(self) -> list[tuple[bool, str]]:
        """Each limit the file is past, `(hard, text)`, code then functions
        then total; a test file is held to the total alone."""
        out: list[tuple[bool, str]] = []
        if not self.is_test:
            if self.code > CODE_HARD:
                out.append((True, f"{self.code} code lines against {CODE_HARD}"))
            elif self.code > CODE_SOFT:
                out.append((False, f"{self.code} code lines against {CODE_SOFT}"))
            if self.fns > FNS_HARD:
                out.append((True, f"{self.fns} functions against {FNS_HARD}"))
            elif self.fns > FNS_SOFT:
                out.append((False, f"{self.fns} functions against {FNS_SOFT}"))
        if self.total > TOTAL_HARD:
            out.append((True, f"{self.total} lines against {TOTAL_HARD}"))
        return out


def read(path: Path, rel: str, is_test: bool) -> Reading:
    text = path.read_text(encoding="utf-8")
    lines = masked(text).split("\n")
    ranges, _ = test_blocks("\n".join(lines))
    in_test = [False] * len(lines)
    for lo, hi in ranges:
        for k in range(lo, hi + 1):
            in_test[k] = True
    if is_test:
        in_test = [True] * len(lines)
    production = "\n".join(line for line, t in zip(lines, in_test) if not t)
    code = sum(1 for line in production.split("\n") if line.strip())
    return Reading(
        path=rel,
        total=len(text.splitlines()),
        code=code,
        fns=len(FN_RE.findall(production)),
        test_lines=len(text.splitlines()) if is_test else sum(in_test),
        is_test=is_test,
    )


def members(repo: Path) -> list[Path]:
    manifest = tomllib.loads((repo / "Cargo.toml").read_text(encoding="utf-8"))
    found: list[Path] = []
    for pattern in manifest["workspace"]["members"]:
        found.extend(p for p in sorted(repo.glob(pattern)) if (p / "Cargo.toml").is_file())
    return found


def rust_files(repo: Path) -> list[Path]:
    files: list[Path] = []
    for member in members(repo):
        files.extend(
            p for p in sorted(member.rglob("*.rs")) if "target" not in p.relative_to(repo).parts
        )
    return files


def readings(repo: Path) -> list[Reading]:
    files = rust_files(repo)
    declared: set[Path] = set()
    for path in files:
        _, names = test_blocks(masked(path.read_text(encoding="utf-8")))
        declared.update(declared_test_files(path, names))
    out = []
    for path in files:
        rel = path.relative_to(repo).as_posix()
        is_test = path in declared or "tests" in path.relative_to(repo).parts
        out.append(read(path, rel, is_test))
    return out


def problems(found: list[Reading]) -> tuple[list[str], list[str], list[str], list[str]]:
    """Hard lines, soft lines, exempt lines, stale-exemption lines."""
    hard, soft, exempt, stale = [], [], [], []
    seen = set()
    for r in sorted(found, key=lambda r: (-r.code, r.path)):
        over = r.over()
        texts = ", ".join(text for _, text in over)
        past_hard = any(is_hard for is_hard, _ in over)
        if r.path in EXEMPT:
            seen.add(r.path)
            if past_hard:
                exempt.append(f"== {r.path}: exempt, {texts} -- {EXEMPT[r.path]}")
            else:
                stale.append(f"!! stale exemption: {r.path} is under every hard limit")
            continue
        if past_hard:
            hard.append(f"!! {r.path}: {texts}")
        elif over:
            soft.append(f".. {r.path}: {texts}")
    for path in sorted(set(EXEMPT) - seen):
        stale.append(f"!! stale exemption: {path} is not in the workspace")
    return hard, soft, exempt, stale


def check(repo: Path, out: TextIO, show_all: bool = False) -> int:
    found = readings(repo)
    if show_all:
        print(f"{'code':>6} {'fns':>5} {'tests':>6} {'total':>6}  path", file=out)
        for r in sorted(found, key=lambda r: (-r.code, -r.total)):
            tag = "  (test file)" if r.is_test else ""
            print(f"{r.code:6} {r.fns:5} {r.test_lines:6} {r.total:6}  {r.path}{tag}", file=out)
    hard, soft, exempt, stale = problems(found)
    for line in hard + soft + exempt + stale:
        print(line, file=out)
    verdict = (
        f"file-limits: {len(hard)} past a hard limit, {len(soft)} past a soft one, "
        f"{len(exempt)} exempt"
    )
    if stale:
        print(f"{verdict}; {len(stale)} stale exemptions", file=out)
        return 2
    print(f"{verdict}; {'seams' if hard else 'green'}", file=out)
    return 1 if hard else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--all", action="store_true", help="print every file's reading")
    args = parser.parse_args(argv)
    return check(REPO, sys.stdout, show_all=args.all)


if __name__ == "__main__":
    sys.exit(main())
