"""Comment-only commits: the one oracle the harness computes for itself."""

from __future__ import annotations

import functools
import re

from measurement import git


#: Suffixes the comment oracle can read. Anything else -- a manifest, a SQL
#: fixture, a snapshot -- is unanalyzable, and unanalyzable is a synonym for
#: "not comment-only": the oracle's whole value is that it never has to be
#: trusted, so every uncertainty resolves against the skip.
_COMMENTABLE = (".rs", ".py", ".md")


def _rust_comment_lines(text: str) -> set[int] | None:
    """The 1-based lines of a Rust source that are comment or blank.

    `None` means the file defeated the scanner, which is the conservative
    answer everywhere: a block comment opened after code on the same line, or
    a `/*` inside a string literal, would need a real lexer to place, and a
    wrong answer here excuses a change that moved a number. Line comments --
    `//`, `///`, `//!` -- are the case this project actually writes, and a
    whole-line `/* ... */` block is read as well."""
    out: set[int] = set()
    depth = 0
    for n, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if depth:
            out.add(n)
            depth += line.count("/*") - line.count("*/")
            if depth < 0:
                return None
            continue
        if not line or line.startswith("//"):
            out.add(n)
            continue
        if line.startswith("/*"):
            out.add(n)
            depth += line.count("/*") - line.count("*/")
            if depth < 0:
                return None
            continue
        if "/*" in line or "*/" in line:
            # Code and a block delimiter on one line: where the comment starts
            # and ends is a lexing question, so the file is not read at all.
            return None
    return out if depth == 0 else None

#: A triple-quoted region counts as a comment only where it *opens* like a
#: docstring -- the line is nothing but the quote, or the quote plus prose.
#: `x = """..."""` is a value, and a value is code.
_PY_DOCSTRING_OPEN = re.compile(r"""^(?:[rRbBuUfF]{0,2})("{3}|'{3})""")


def _python_comment_lines(text: str) -> set[int] | None:
    """The 1-based lines of a Python source that are comment, docstring or blank."""
    out: set[int] = set()
    closer: str | None = None
    for n, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if closer is not None:
            out.add(n)
            if closer in line:
                if line.count(closer) > 1:
                    return None
                closer = None
            continue
        if not line or line.startswith("#"):
            out.add(n)
            continue
        m = _PY_DOCSTRING_OPEN.match(line)
        if not m:
            if '"""' in line or "'''" in line:
                # A triple quote arriving after code: an assigned literal, a
                # nested quote, or the end of something this scanner never saw
                # open. All three are beyond it.
                return None
            continue
        quote = m.group(1)
        out.add(n)
        rest = line[m.end():]
        if quote not in rest:
            closer = quote
        elif rest.count(quote) > 1:
            return None
    return out if closer is None else None


def comment_lines(path: str, text: str) -> set[int] | None:
    """Which of a blob's lines carry nothing a build can see.

    Markdown is *entirely* comment: it compiles to nothing and runs in no
    figure's command shape, so every line of it qualifies and the scanner is
    an arithmetic one."""
    if path.endswith(".md"):
        return set(range(1, len(text.splitlines()) + 1))
    if path.endswith(".rs"):
        return _rust_comment_lines(text)
    if path.endswith(".py"):
        return _python_comment_lines(text)
    return None

_HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def hunk_ranges(diff: str) -> tuple[list[int], list[int]]:
    """The pre-image and post-image line numbers a `-U0` diff touches.

    Read off the hunk headers rather than by counting `+`/`-` lines: with no
    context there is nothing else in a hunk, and a header states both sides'
    ranges outright."""
    removed: list[int] = []
    added: list[int] = []
    for line in diff.splitlines():
        m = _HUNK_RE.match(line)
        if not m:
            continue
        astart, alen, bstart, blen = m.groups()
        alen = 1 if alen is None else int(alen)
        blen = 1 if blen is None else int(blen)
        removed += list(range(int(astart), int(astart) + alen))
        added += list(range(int(bstart), int(bstart) + blen))
    return removed, added


@functools.lru_cache(maxsize=None)
def comment_only_commit(commit: str, path: str) -> bool:
    """Does this commit change nothing but comments, inside this one path?

    **The third oracle, computed rather than written down.** It used to be a
    claim an acknowledgement entry made -- and three of seven consecutive
    commits on `main` existed only to make it, each one a commit whose own
    diff was comments retargeting citations. The property is syntactic, so the
    harness decides it: every line the commit adds falls inside a comment of
    the post-image, every line it removes fell inside a comment of the
    pre-image, and a file the scanner cannot place is not comment-only.

    Refused before the scanner runs: a merge, which has no single pre-image; a
    file added, deleted or renamed, where "the lines it changed" is the whole
    of it; and a suffix the oracle does not read."""
    if not path.endswith(_COMMENTABLE):
        return False
    sha = git._git(["rev-parse", f"{commit}^{{commit}}"])
    if sha is None:
        return False
    sha = sha.strip()
    parents = git._git(["rev-list", "--parents", "-n", "1", sha])
    if parents is None or len(parents.split()) != 2:
        return False
    status = git._git(["show", "--format=", "--name-status", "-M", sha, "--", path])
    if status is None:
        return False
    kinds = {line.split("\t")[0][:1] for line in status.splitlines() if line.strip()}
    if kinds and kinds != {"M"}:
        return False
    diff = git._git(["show", "--format=", "--unified=0", sha, "--", path])
    if not diff:
        # No hunks under this path at all: nothing to excuse, and saying
        # "comment-only" about a path the commit did not touch would be a
        # claim nobody asked for.
        return False
    removed, added = hunk_ranges(diff)
    before = git._git(["show", f"{sha}^:{path}"])
    after = git._git(["show", f"{sha}:{path}"])
    if before is None or after is None:
        return False
    was = comment_lines(path, before)
    now = comment_lines(path, after)
    if was is None or now is None:
        return False
    return all(n in was for n in removed) and all(n in now for n in added)
