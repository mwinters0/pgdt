#!/usr/bin/env python3
"""The citation check: every `<doc>.md`, "section" reference in the tree
resolved against the section it names.

The project cites a document's *section*, not the whole document -- a habit that
is most of what makes a 4,000-line `decisions.md` navigable, and that nothing
verified until this. A citation is a pointer, and `docs/process.md`, "The test
each artifact must pass" is blunt about what a broken one costs: a session
spends a tool call following it and finds nothing, which is worse than having no
pointer at all. Headings are rewritten as the things under them move,
so the discipline decays quietly and in exactly the places nobody re-reads --
three of the nine this first reported sit in phase *inboxes*, which are read
at grilling time, the moment a stale pointer costs most.

**A dated entry whose day has closed is exempt**, and that is a property of what
history *is* rather than a concession: an entry states what was true on its date
and is not maintained, so a stale pointer in one is not a defect. See
`reads_citations`, which holds the whole of that rule.

Three resolutions, and they are deliberately not one rule:

* **A `section:` id resolves strictly**, by set membership against the ids
  declared in the tree. It is matched whether the citation names its document
  (`` (`docs/design/decisions.md`, "D29") ``) or quotes the id bare,
  which is the idiom a notes doc uses when the document is named once and
  several of its sections are then quoted in a list. Bare resolution is by
  membership rather than by scoping the document name to the sentence: a second
  grammar there has a false-positive surface over every quoted phrase in the
  tree, where an id is kebab-case and cannot collide with English.
* **A heading or a bold paragraph lead resolves leniently** -- exactly, or as a
  truncated leading clause of one. That grammar is what the project actually
  writes, and a strict match over it cries wolf: a citation legitimately names
  the first clause of a long heading, and `process.md`'s "Slice numbering" and
  "Decisions worth another look" are bold `- **…**` list items rather than `#`
  headings. A matcher ignorant of either reports a hundred dangling citations,
  every one spurious.
* **A marked section cited by its heading fails**, naming the id to use
  instead. Lenience there would let the marker's whole purpose lapse silently:
  the heading is still in the file, so a lenient matcher resolves it happily and
  the citation breaks at the next re-measure -- which is the failure the marker
  exists to prevent, reached by the one route that leaves no trace.

**A citation is something written to a reader, so this reads comments and prose
and never string literals.** That is what keeps `scripts/measure.py`'s real
citations -- which live in its docstrings and its `#:` comments -- in scope
while `test_deficiencies.py`'s synthetic fixtures and every `.md` path literal
stay out. Excluding `test_*.py` by name was the alternative and is escaped
silently by the next test file; editing the fixtures to name a doc that does not
exist makes them lie about what they test in order to satisfy an unrelated
check.

**The grammar has to survive how the citations are actually laid out**, and each
of these was a real miss in a prototype rather than a hypothetical:

* Citations **wrap across lines** -- `postgres-invariants.md` names the document
  on one line and quotes the section on the next -- so the text is joined before
  it is matched, with a sentinel where the prose stops so that a citation can
  never be assembled across a gap of code.
* A doc comment's **leader sits inside the citation**: `///` and `//!` wrap
  mid-quote in six `.rs` files, so the leader is stripped per line before the
  join.
* A **quoted target carries inline markup and wraps**: `docs/process.md`,
  "Phase identity is `P<k>`" spans two lines and holds a code span. Both sides
  are normalised -- markup stripped, whitespace collapsed, trailing punctuation
  dropped -- before they are compared.
* **One citation may name several sections**, `"Predicates" and "The CLI"`.
  Each is resolved; taking only the first would let the second rot unwatched.
* A **blockquote's leader** does the same thing a doc comment's does, and is
  stripped the same way.
* A **heading may hold a quoted phrase**, which a citation -- itself a quoted
  string -- cannot nest, so quotation marks are normalised away on both sides.
* A citation may **elide its leading clause** behind an ellipsis where the
  sentence has already said it.
* A document is often named **only by a link's target**, the link's text being a
  date. Every row of the out-of-band ledger is written that way.

**This fails rather than reports, and there is no acknowledged-baseline file.**
A second register to keep honest costs more than the short window of red that
the repairs close. If those were to slip, what that earns is a known-deficiency
entry, not a mechanism for living with the red.

**The one thing it cannot see** is a bare id citation whose id has been deleted:
membership is the resolution, so the citation reverts to being an ordinary
quoted phrase. The doc-named form is checked -- the document is named, so the
id must be in *that* document's set -- which is the shape to prefer where the
distinction matters.

Usage:

    cd scripts
    uv run citations.py
    uv run python -m unittest test_citations
"""

from __future__ import annotations

import argparse
import ast
import bisect
import datetime as dt
import io as _io
import os
import re
import sys
import tokenize
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Iterable, Sequence

REPO = Path(__file__).resolve().parent.parent

#: Markdown that is both a source of citations and a target of them. Every
#: directory here is checked in; `CLAUDE.local.md` is deliberately absent,
#: since it is gitignored and describes one machine, so reading it would make
#: the check's result depend on whose checkout it runs in.
DOC_ROOTS = ("docs", ".claude/skills")
ROOT_DOCS = ("README.md", "CLAUDE.md", "CONTRIBUTING.md")

#: Documents that may be cited but cannot be resolved: `CLAUDE.local.md` is
#: gitignored, so its sections exist on one machine and nowhere else. A citation
#: of one is accepted whole rather than reported, which is the honest reading --
#: the target is real and this checkout simply cannot see it.
UNCHECKED_DOCS = ("CLAUDE.local.md",)

#: Dated entries. An entry states what was true on its date and is not
#: maintained afterwards (`docs/status/history/README.md`, "An entry carries
#: yesterday's truth"), so a keystone that deletes a phase doc leaves every
#: history citation of it dangling *by rule* -- `docs/process.md`, "What a
#: keystone review must not do", forbids rewriting them -- and repairing one is
#: not owed. Their **headings are still resolvable targets**: the out-of-band
#: ledger's `Why` column cites a dated entry by heading, so the exclusion is on
#: the reading side alone and never on `documents()`.
HISTORY_DIR = "docs/status/history/"

#: `YYYY-MM-DD.md`, the shape of a dated entry. `README.md` beside them states
#: the rules rather than a day's facts, so it is read like any other document.
DATED_ENTRY_RE = re.compile(r"^\d{4}-\d{2}-\d{2}\.md$")

#: Rust: sources only. Comments and doc comments, never code.
RUST_ROOTS = ("pgdump_query", "pgdt", "datafusion-pgdump", "datafusion-cli-pgdump")

#: Python: sources only. Docstrings and comments, never string literals.
PYTHON_ROOTS = ("scripts",)

#: A line the extractor refused. It stops `\s*` from joining a citation across
#: the gap, which a blank line would not.
GAP = "\x00"

#: `<!-- section: <id> -->` on the line above the heading it names. The id --
#: not the heading -- is what a citation of that section names, so the heading
#: above it is free to state a finding and be rewritten when the finding moves.
SECTION_MARKER_RE = re.compile(r"<!--\s*section:\s*([a-z0-9][a-z0-9-]*)\s*-->")

#: An id, for recognising one quoted bare. Kebab-case with at least one hyphen,
#: which is what keeps it from colliding with an English phrase.
ID_SHAPE_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)+$")

HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*$", re.M)

#: A bold lead: the `**…**` run opening a paragraph, a list item or a quote.
#: The project cites these as sections and they are frequently `- **…**` rows,
#: so anchoring `**` to the start of a line calls four live citations dangling.
LEAD_RE = re.compile(r"^[ \t]*(?:[-*+]\s+|>\s*)?\*\*(.+?)\*\*", re.M | re.S)

#: The section a quoted target may run to. Shared by every citation shape.
_TARGET = r"[\"“]([^\"”\x00]{1,200}?)[\"”]"

#: The named form. The document may be a bare code span or a link whose text is
#: one; the quoted section may follow across a line break.
CITATION_RE = re.compile(
    r"(?:\[\s*)?`([A-Za-z0-9_.\-/]+\.md)`(?:\s*\]\([^)]*\))?\s*,\s*" + _TARGET
)

#: The same citation with the document named only by the **link target** -- a
#: link whose text is a date and whose destination is the dated entry, then the
#: quoted heading, which is what every row of the out-of-band ledger writes. A
#: grammar reading only the link's text sees no document there at all, and those
#: rows are precisely the citations of a *history* entry's heading, which is
#: rewritten in place as the thing it records settles.
LINKED_CITATION_RE = re.compile(
    r"\[[^\]\x00]{0,200}?\]\(\s*([A-Za-z0-9_.\-/]+\.md)\s*\)\s*,\s*" + _TARGET
)

#: `"Predicates" and "The CLI"` -- a second section on the same citation.
MORE_RE = re.compile(r"\s*(?:,\s*)?and\s+[\"“]([^\"”\x00]{1,200}?)[\"”]")

#: Any quoted phrase, for the bare-id form. Resolution is set membership, so a
#: phrase that is not a declared id is simply prose and is never reported.
QUOTED_RE = re.compile(r"[\"“]([^\"”\n\x00]{1,200}?)[\"”]")

#: Fenced code in Markdown. A fence's contents are a transcript or a tree
#: listing, not something written to a reader as a pointer.
FENCE_RE = re.compile(r"^\s*(```|~~~)")

#: Markup a target and a citation are both stripped of before comparison.
LINK_RE = re.compile(r"\[([^\]]*)\]\([^)]*\)")


@dataclass(frozen=True)
class Section:
    """One thing a citation may name."""

    #: Repo-relative path of the document it is in.
    doc: str
    #: "id", "heading" or "lead".
    kind: str
    #: As written.
    text: str
    #: Normalised, for the lenient comparison. Equal to `text` for an id.
    key: str
    line: int
    #: For a heading carrying a `section:` marker, that id. Citing such a
    #: heading is an error rather than a lenient match.
    marker: str | None = None


@dataclass(frozen=True)
class Citation:
    """One reference to a section, as written."""

    #: Repo-relative path of the file it was written in.
    path: str
    line: int
    #: The document named, repo-relative once resolved; None for a bare id.
    doc: str | None
    #: The section named, as written.
    target: str


def repo_rel(path: Path, repo: Path = REPO) -> str:
    try:
        return str(path.relative_to(repo))
    except ValueError:
        return str(path)


def normalize(text: str) -> str:
    """A heading and a citation of it, reduced to what they have in common.

    Inline markup, line wrapping and trailing punctuation all differ between a
    heading and a citation of it without either being wrong, so none of them may
    decide whether the citation resolves.

    **Quotation marks go too**, and that one is not cosmetic: a heading may hold
    a quoted phrase -- `decisions.md`'s *The bar: "the dump alone determines
    the value"* -- and a citation is itself a quoted string, so there is no way
    to write the inner pair. Keeping them would fail a citation whose author had
    no correct form available.
    """
    t = LINK_RE.sub(r"\1", text.replace("\n", " "))
    t = t.replace("`", "").replace("*", "")
    t = re.sub(r"[\"\u201c\u201d]", "", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t.strip(" .:;,—–").lower()


# --------------------------------------------------------------------------
# Extraction: what in a file is prose written to a reader
# --------------------------------------------------------------------------


def _joined(lines: Sequence[str]) -> tuple[str, list[int]]:
    """Join per-line prose into one string, with offsets for line numbers."""
    text = "\n".join(lines)
    starts: list[int] = []
    at = 0
    for line in lines:
        starts.append(at)
        at += len(line) + 1
    return text, starts


def _line_of(starts: Sequence[int], offset: int) -> int:
    return bisect.bisect_right(starts, offset)


#: A blockquote's leader, which wraps mid-citation exactly as a doc comment's
#: does -- `.claude/skills/gosub/SKILL.md` quotes a prompt that cites a section
#: of itself across two `> ` lines.
QUOTE_LEADER_RE = re.compile(r"^\s*>\s?")


def markdown_prose(text: str) -> list[str]:
    """Every line but the fenced code blocks, blockquote leaders stripped."""
    out: list[str] = []
    fence: str | None = None
    for line in text.splitlines():
        opener = FENCE_RE.match(line)
        if fence is None:
            if opener:
                fence = opener.group(1)
                out.append(GAP)
                continue
            out.append(QUOTE_LEADER_RE.sub("", line))
        else:
            out.append(GAP)
            if opener and opener.group(1) == fence:
                fence = None
    return out


RUST_LEADER_RE = re.compile(r"^\s*(///|//!|//)\s?")


def rust_prose(text: str) -> list[str]:
    """Comment lines, leaders stripped. Code contributes a gap.

    Block comments are not read. `pg_dump`'s own `/* … */` appears in this tree
    only inside string literals and doc comments, and a comment style the code
    does not use is not worth a second parser.
    """
    out: list[str] = []
    for line in text.splitlines():
        m = RUST_LEADER_RE.match(line)
        out.append(line[m.end():] if m else GAP)
    return out


def python_prose(text: str) -> list[str]:
    """Docstrings and comments. Every other line contributes a gap.

    A docstring is a string literal to the interpreter and prose to a reader,
    and the distinction is exactly what this check needs: `measure.py` writes
    its citations in docstrings and `#:` comments, while the synthetic doc names
    in `test_deficiencies.py` are ordinary literals and stay out of scope.
    """
    lines = text.splitlines()
    out = [GAP] * len(lines)

    try:
        tree = ast.parse(text)
    except SyntaxError:
        tree = None
    if tree is not None:
        for node in ast.walk(tree):
            if not isinstance(
                node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
            ):
                continue
            body = getattr(node, "body", None)
            if not body:
                continue
            first = body[0]
            if not (
                isinstance(first, ast.Expr)
                and isinstance(first.value, ast.Constant)
                and isinstance(first.value.value, str)
            ):
                continue
            for i in range(first.lineno - 1, min(first.end_lineno, len(lines))):
                out[i] = lines[i].replace('"""', "   ").replace("'''", "   ")

    try:
        tokens = tokenize.generate_tokens(_io.StringIO(text).readline)
        for tok in tokens:
            if tok.type == tokenize.COMMENT:
                row = tok.start[0] - 1
                if 0 <= row < len(out):
                    body = tok.string.lstrip("#")
                    out[row] = body[1:] if body.startswith(":") else body
    except (tokenize.TokenError, IndentationError, SyntaxError):
        pass
    return out


def prose_of(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix == ".md":
        return markdown_prose(text)
    if path.suffix == ".rs":
        return rust_prose(text)
    if path.suffix == ".py":
        return python_prose(text)
    return []


# --------------------------------------------------------------------------
# Targets: what a document offers to be cited by
# --------------------------------------------------------------------------


def parse_sections(doc: str, text: str) -> list[Section]:
    """Every id, heading and bold lead a document offers."""
    lines = text.splitlines()
    marked: dict[int, str] = {}
    pending: str | None = None
    for i, line in enumerate(lines):
        m = SECTION_MARKER_RE.search(line)
        if m:
            pending = m.group(1)
            continue
        if pending is not None:
            if not line.strip():
                continue
            if line.lstrip().startswith("#"):
                marked[i] = pending
            pending = None

    out: list[Section] = []
    for line, ident in sorted(marked.items()):
        out.append(Section(doc, "id", ident, ident, line + 1, marker=ident))

    body, starts = _joined(lines)
    for m in HEADING_RE.finditer(body):
        line = _line_of(starts, m.start())
        raw = m.group(2)
        out.append(
            Section(doc, "heading", raw, normalize(raw), line, marked.get(line - 1))
        )
    for m in LEAD_RE.finditer(body):
        raw = m.group(1)
        if "\n\n" in raw:
            # A `**` that never closed inside its own block: the run swallowed a
            # paragraph break, so it is emphasis in running prose, not a lead.
            continue
        out.append(Section(doc, "lead", raw, normalize(raw), _line_of(starts, m.start())))
    return out


# --------------------------------------------------------------------------
# Citations
# --------------------------------------------------------------------------


def citations_in(path: str, lines: Sequence[str], ids: Iterable[str]) -> list[Citation]:
    """Every citation written in one file's prose."""
    body, starts = _joined(lines)
    known = set(ids)
    out: list[Citation] = []
    seen: set[tuple[int, int]] = set()
    for pattern in (CITATION_RE, LINKED_CITATION_RE):
        for m in pattern.finditer(body):
            # The two shapes overlap wherever a link's text *is* the document
            # name, which is the commonest form in the tree.
            if (m.start(2), m.end(2)) in seen:
                continue
            seen.add((m.start(2), m.end(2)))
            doc = m.group(1)
            out.append(Citation(path, _line_of(starts, m.start()), doc, m.group(2)))
            at = m.end()
            while True:
                more = MORE_RE.match(body, at)
                if not more:
                    break
                seen.add((more.start(1), more.end(1)))
                out.append(
                    Citation(path, _line_of(starts, more.start()), doc, more.group(1))
                )
                at = more.end()
    for m in QUOTED_RE.finditer(body):
        target = m.group(1).strip()
        if target in known and ID_SHAPE_RE.match(target):
            out.append(Citation(path, _line_of(starts, m.start()), None, target))
    return out


# --------------------------------------------------------------------------
# Resolution
# --------------------------------------------------------------------------


def resolve_doc(named: str, frm: str, docs: dict[str, list[Section]]) -> str | None:
    """The document a citation's `<doc>.md` names, repo-relative.

    A citation names it by whatever reads well at the site: the link-relative
    `../design/decisions.md` in a status doc, the repo-relative path in a
    `.rs` doc comment, the bare basename in a notes doc beside it. So three
    resolutions are tried in that order, and an ambiguous basename resolves to
    nothing and is reported rather than guessed.
    """
    here = PurePosixPath(frm).parent
    for candidate in (here / named, PurePosixPath(named)):
        try:
            rel = str(PurePosixPath(os.path.normpath(str(candidate))))
        except ValueError:  # pragma: no cover - normpath does not raise here
            continue
        if rel in docs:
            return rel
    hits = [d for d in docs if d.endswith("/" + named)]
    if len(hits) == 1:
        return hits[0]
    return None


ELLIPSIS_RE = re.compile(r"^\s*(?:\u2026|\.\.\.)\s*")


def _boundary(text: str, at: int) -> bool:
    """Whether an offset into a normalised key falls between words."""
    return at <= 0 or at >= len(text) or not text[at].isalnum()


def _match(
    key: str, elided: bool, sections: Sequence[Section], loose: bool
) -> Section | None:
    """The section a heading-or-lead citation names, or None.

    `loose` admits a **truncated leading clause** -- the citation names the head
    of a long heading and stops at a word boundary, so "Predicate" is not a
    citation of "Predicates". It is a second pass rather than one rule so that
    an exact match elsewhere in the document always wins over a prefix of a
    longer heading.

    Where the citation opens with an ellipsis the elision is at the *front*
    instead: `measurements.md`'s two census tables are cited as "The census on
    brace-free rows" and "…on array-bearing rows", the second standing in for a
    prefix the sentence has already said. That is matched as an interior run
    bounded at both ends, which is what an ellipsis claims and no more, so it
    ignores `loose` entirely.
    """
    for s in sections:
        if s.kind == "id":
            continue
        if elided:
            at = s.key.find(key)
            if at >= 0 and _boundary(s.key, at - 1) and _boundary(s.key, at + len(key)):
                return s
        elif s.key == key:
            return s
        elif loose and s.key.startswith(key) and _boundary(s.key, len(key)):
            return s
    return None


def reconcile(
    citations: Sequence[Citation],
    docs: dict[str, list[Section]],
    ids: dict[str, str],
) -> list[str]:
    problems: list[str] = []
    for c in citations:
        if c.doc is None:
            # A bare id, resolved by membership: it is a citation because the id
            # exists, so there is nothing left to fail.
            continue
        if c.doc in UNCHECKED_DOCS:
            continue
        doc = resolve_doc(c.doc, c.path, docs)
        if doc is None:
            problems.append(
                f"{c.path}:{c.line} cites `{c.doc}`, which names no document in "
                f"the tree — a relative path is read from {c.path}'s own "
                f"directory, as a link in it would be"
            )
            continue
        sections = docs[doc]
        target = re.sub(r"\s+", " ", c.target).strip()
        if any(s.kind == "id" and s.text == target for s in sections):
            continue
        elided = bool(ELLIPSIS_RE.match(target))
        key = normalize(ELLIPSIS_RE.sub("", target))
        # A marked section is resolved apart from the rest, and its heading is a
        # failure at either strictness. Letting a *truncated* clause of one match
        # leniently would leave the marker exactly as worthless as letting the
        # whole heading match: the citation still names a string that is rewritten
        # at the next re-measure.
        marked = [s for s in sections if s.kind == "heading" and s.marker]
        plain = [s for s in sections if not (s.kind == "heading" and s.marker)]
        hit = None
        for loose in (False, True):
            if _match(key, elided, plain, loose) is not None:
                hit = "ok"
                break
            found = _match(key, elided, marked, loose)
            if found is not None:
                problems.append(
                    f'{c.path}:{c.line} cites {doc} by the heading "{target}", '
                    f"which carries a `section:` marker — cite "
                    f'"{found.marker}" instead, since the heading is rewritten '
                    f"whenever the finding under it moves"
                )
                hit = "marked"
                break
        if hit is not None:
            continue
        if ID_SHAPE_RE.match(target) and target in ids:
            problems.append(
                f'{c.path}:{c.line} cites {doc}, "{target}" — that id is declared '
                f"in {ids[target]}, not there"
            )
            continue
        problems.append(
            f'{c.path}:{c.line} cites {doc}, "{target}" — no section of it '
            f"resolves, by id, heading or bold lead"
        )
    return problems


# --------------------------------------------------------------------------
# Walking the tree
# --------------------------------------------------------------------------


def sources(repo: Path = REPO) -> list[Path]:
    """Every file whose prose is read for citations, deterministically ordered."""
    out: list[Path] = []
    for name in ROOT_DOCS:
        p = repo / name
        if p.exists():
            out.append(p)
    for root in DOC_ROOTS:
        out.extend(sorted((repo / root).rglob("*.md")))
    for root in RUST_ROOTS:
        out.extend(sorted((repo / root).rglob("*.rs")))
    for root in PYTHON_ROOTS:
        out.extend(sorted((repo / root).rglob("*.py")))
    return [p for p in out if p.is_file()]


def reads_citations(rel: str, today: str) -> bool:
    """Whether a file's own citations are resolved.

    Every file's, except a dated entry whose day has closed. The two failures
    are not the same age: a mistyped path or a section named by a title its
    target has never carried is wrong the *moment* it is written, while a
    pointer only goes stale when something else deletes or renames its target,
    which takes days. So today's entry is still read -- the session writing it
    gets its own pointers resolved -- and no entry is ever re-litigated after
    its date, which is what keeps the red count from growing with every
    keystone. A permanently red check stops being read.
    """
    if not rel.startswith(HISTORY_DIR):
        return True
    name = rel[len(HISTORY_DIR) :]
    if not DATED_ENTRY_RE.match(name):
        return True
    return name == f"{today}.md"


def documents(repo: Path = REPO) -> dict[str, list[Section]]:
    """Every Markdown document a citation may name, and what it offers."""
    out: dict[str, list[Section]] = {}
    for p in sources(repo):
        if p.suffix != ".md":
            continue
        rel = repo_rel(p, repo)
        out[rel] = parse_sections(rel, p.read_text(encoding="utf-8", errors="replace"))
    return out


def collect(
    repo: Path = REPO, today: str | None = None
) -> tuple[list[Citation], dict[str, list[Section]], dict[str, str], list[str], int]:
    if today is None:
        today = dt.date.today().isoformat()
    docs = documents(repo)
    ids: dict[str, str] = {}
    problems: list[str] = []
    for doc, sections in sorted(docs.items()):
        for s in sections:
            if s.kind != "id":
                continue
            if s.text in ids:
                problems.append(
                    f"{doc}:{s.line} declares `section: {s.text}`, which "
                    f"{ids[s.text]} already declares — a bare citation of it "
                    f"resolves by set membership and cannot say which"
                )
                continue
            if not ID_SHAPE_RE.match(s.text):
                # A one-word id cannot be quoted bare: set membership is the
                # only thing separating an id from an English phrase, and
                # kebab-case is what makes that separation safe.
                problems.append(
                    f"{doc}:{s.line} declares `section: {s.text}`, which is not "
                    f"kebab-case — a bare citation of it would be "
                    f"indistinguishable from an ordinary quoted phrase"
                )
                continue
            ids[s.text] = doc
    citations: list[Citation] = []
    read = 0
    for p in sources(repo):
        rel = repo_rel(p, repo)
        if not reads_citations(rel, today):
            continue
        read += 1
        citations.extend(citations_in(rel, prose_of(p), ids))
    return citations, docs, ids, problems, read


def report(
    citations: Sequence[Citation],
    docs: dict[str, list[Section]],
    ids: dict[str, str],
    problems: Sequence[str],
    files: int = 0,
    out=sys.stdout,
) -> None:
    named = [c for c in citations if c.doc is not None]
    bare = len(citations) - len(named)
    targets = len({(c.doc, c.target) for c in named})
    print(
        f"{len(citations)} citations read in {files} files: "
        f"{len(named)} naming a document across {targets} distinct targets, "
        f"{bare} a `section:` id bare.",
        file=out,
    )
    print(f"{len(ids)} ids declared, {len(docs)} documents.\n", file=out)
    for ident, doc in sorted(ids.items()):
        uses = sum(1 for c in citations if c.target == ident)
        print(f"  {ident:<22}{doc}  ({uses} cited)", file=out)
    print(file=out)
    if problems:
        print("Citations that do not resolve:", file=out)
        for p in problems:
            print(f"  {p}", file=out)
    else:
        print("Every citation resolves to the section it names.", file=out)


def check(repo: Path = REPO, out=sys.stdout, today: str | None = None) -> int:
    citations, docs, ids, problems, read = collect(repo, today)
    problems = list(problems) + reconcile(citations, docs, ids)
    report(citations, docs, ids, problems, read, out=out)
    return 1 if problems else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo", type=Path, default=REPO, help="repository root (tests use this)"
    )
    args = parser.parse_args(argv)
    return check(args.repo)


if __name__ == "__main__":
    raise SystemExit(main())
