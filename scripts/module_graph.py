#!/usr/bin/env python3
"""The module graph: which module's code reads which, taken from
rust-analyzer's index of the workspace and held one-way, with a design's
moves applied to it before any code moves.

The rule is `docs/process.md`, "Seams": every edge between modules runs one
way, inside a crate as Cargo makes them run between crates. A module's name
and concept say where it should sit, but only its references say which way
its edges run. A design drawn from the first without the second is how a cut
lands a two-way edge that its design called one-way. So edges are computed,
never assumed:

* a **module** is a production file of a workspace member: its module tree
  from `lib.rs` or `main.rs`, as `layering.module_tree` reads it, named
  `<package>::<path>`. An inline `mod {}` counts as part of its file;
* an **edge** A -> B is a reference in A's production code to an item B
  defines, as rust-analyzer resolves it, method calls, trait impls and
  `Self` included. A `use` declaration is not one: a re-export is how a
  namespace declares its children's API, and an import counts where it is
  used. Nor is a reference in `#[cfg(test)]` code, or a module named as a
  path segment;
* a **cycle** is a set of modules that reach each other, a parent and its
  child included. Each one is a problem unless `ACCEPTED` lists its modules
  with the reason it is kept, or `OWED` lists them with the work that
  removes it. An entry matching no cycle exactly is stale, which is a
  problem too, so a cut that shrinks a cycle rewrites its entry.

**A design is checked before it is cut** (`--design <doc>`). The moves in
the doc's ```` ```moves ```` blocks are applied to the current references,
and the cycles the moved tree would have are reported, each with the
references that make it. A line `<from> -> <to>` moves one item
(`pgdt-codec::grammar::column_grammar`), one method of an item's `impl`s
(`pgdt-query::predicate::ResolvedTerm::eval`), or every item a module's own
file defines (`pgdt-codec::key::order`); a method's line wins over its item's,
and an item's over its module's. `<to>` is a module, existing or new. An item
is a top-level definition, and an `impl`'s methods move with its self type
unless a line names one: a type whose impls are split by role, each beside
the role's other items. A cycle
spanning crates is one Cargo would refuse, and is reported as a crate
cycle.

**Doc links are not edges**: an explanation may lean on whatever it is
explained against. But a doc link that runs against the code's edges, one
module's doc naming a module that reads it, says the concept does not yet
stand on its own. `--docs` lists those links, and they never fail the check.
A doc link here is a `crate::`, `super::`, `self::` or `pgdt_*::` path in a
comment, since rust-analyzer's index carries none.

rust-analyzer is the `rust-toolchain.toml` component that `rustup which
rust-analyzer` names. Its `scip` index covers the whole workspace, so a
design iterated against an unchanged tree reads a saved index (`--index`,
written by `--save-index`).

Usage:

    cd scripts && uv run module_graph.py [--docs] [--design <doc>]
    cd scripts && uv run python -m unittest test_module_graph
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator, TextIO

import layering
from file_limits import _close_brace, masked, test_blocks

REPO = Path(__file__).resolve().parent.parent

#: Cycles kept by design, each with its reason, keyed by the cycle's modules.
ACCEPTED: dict[frozenset[str], str] = {
    frozenset({"pgdt-io::source::origin", "pgdt-io::source::remote"}): (
        "a dispatcher and its `http` arm: the arm answers the probe the dispatcher defines"
    ),
}

#: Cycles the tree has and a design or a slice removes, each naming which;
#: a phase's closing seams round leaves none.
OWED: dict[frozenset[str], str] = {}

#: How many references are shown for each edge of a cycle.
WITNESSES = 3

# ---------------------------------------------------------------------------
# The index: SCIP's protobuf, decoded for the fields this reads.


def _varint(buf: bytes, i: int) -> tuple[int, int]:
    out = shift = 0
    while True:
        b = buf[i]
        i += 1
        out |= (b & 0x7F) << shift
        shift += 7
        if b < 0x80:
            return out, i


def _fields(buf: bytes) -> Iterator[tuple[int, int | bytes]]:
    i = 0
    while i < len(buf):
        key, i = _varint(buf, i)
        number, wire = key >> 3, key & 7
        if wire == 0:
            value, i = _varint(buf, i)
        elif wire == 2:
            n, i = _varint(buf, i)
            value, i = buf[i : i + n], i + n
        elif wire == 1:
            value, i = buf[i : i + 8], i + 8
        elif wire == 5:
            value, i = buf[i : i + 4], i + 4
        else:
            raise ValueError(f"protobuf wire type {wire} at byte {i}")
        yield number, value


def _packed(buf: bytes) -> list[int]:
    out, i = [], 0
    while i < len(buf):
        v, i = _varint(buf, i)
        out.append(v)
    return out


Position = tuple[int, int]  # 0-based line, UTF-8 byte column


@dataclass(frozen=True)
class Occurrence:
    start: Position
    symbol: str
    definition: bool
    enclosing: tuple[Position, Position] | None = None


@dataclass
class Document:
    path: str  # relative to the indexed root
    occurrences: list[Occurrence]


def _span(r: list[int]) -> tuple[Position, Position]:
    # SCIP's range: [line, start, end] on one line, or [line, start, line, end].
    if len(r) == 3:
        return (r[0], r[1]), (r[0], r[2])
    return (r[0], r[1]), (r[2], r[3])


def read_index(data: bytes) -> list[Document]:
    """An index's documents: `Index.documents` (2); `Document.relative_path`
    (1) and `.occurrences` (2); `Occurrence.range` (1), `.symbol` (2),
    `.symbol_roles` (3, bit 0 a definition) and `.enclosing_range` (7)."""
    docs: list[Document] = []
    for number, value in _fields(data):
        if number != 2:
            continue
        path, occurrences = "", []
        for dn, dv in _fields(value):
            if dn == 1:
                path = dv.decode()
            elif dn == 2:
                o = dict(_fields(dv))
                if 1 not in o or 2 not in o:
                    continue
                enclosing = _span(_packed(o[7])) if 7 in o else None
                occurrences.append(
                    Occurrence(
                        _span(_packed(o[1]))[0],
                        o[2].decode(),
                        bool(int(o.get(3, 0)) & 1),
                        enclosing,
                    )
                )
        docs.append(Document(path, occurrences))
    return docs


def build_index(repo: Path, out: Path) -> None:
    """rust-analyzer's `scip` index of the workspace, written to `out`."""
    which = subprocess.run(
        ["rustup", "which", "rust-analyzer"], cwd=repo, capture_output=True, text=True
    )
    if which.returncode != 0:
        raise RuntimeError(
            "no rust-analyzer: it is `rust-toolchain.toml`'s component "
            f"(`rustup component add rust-analyzer`): {which.stderr.strip()}"
        )
    subprocess.run(
        [which.stdout.strip(), "scip", str(repo), "--output", str(out)],
        cwd=repo,
        check=True,
        capture_output=True,
    )


# ---------------------------------------------------------------------------
# Symbols: `rust-analyzer cargo <package> <version> <descriptors>`.

_NAME_RE = re.compile(r"`(?:[^`]|``)*`|[^/#.()\[\]!:`\s]+")


def descriptors(symbol: str) -> str | None:
    """A global symbol's descriptors, or None for a local."""
    if symbol.startswith("local "):
        return None
    parts = symbol.split(" ", 4)
    return parts[4] if len(parts) == 5 else None


def is_module(symbol: str) -> bool:
    d = descriptors(symbol)
    return d is not None and d.endswith("/")


def item_name(symbol: str) -> str | None:
    """The top-level item a symbol belongs to: its first descriptor after the
    module path, an `impl`'s self type for what an `impl` holds."""
    d = descriptors(symbol)
    if d is None:
        return None
    i = 0
    while i < len(d):
        m = _NAME_RE.match(d, i)
        if m is None:
            return None
        name, i = m.group(0).strip("`"), m.end()
        suffix = d[i : i + 1]
        if suffix == "/":
            i += 1
            continue
        if name == "impl" and suffix == "#":
            bracket = re.match(r"#\[([^\]]*)\]", d[i:])
            return bracket.group(1).strip("`") if bracket else None
        return name
    return None


# ---------------------------------------------------------------------------
# The workspace's production files.

USE_RE = re.compile(r"\b(?:pub(?:\s*\([^)]*\))?\s+)?use\s+[^;]*;")
IMPL_RE = re.compile(r"(?m)^[ \t]*(?:unsafe\s+)?impl\b")
DOC_PATH_RE = re.compile(r"\b(crate|super|self|pgdt_\w+|datafusion_\w+)((?:::\w+)+)")
TOP = "(top level)"
TEST = "\0"


def _blank_span(lines: list[list[str]], text: str, lo: int, hi: int, mark: str = " ") -> None:
    line = text.count("\n", 0, lo)
    col = lo - (text.rfind("\n", 0, lo) + 1)
    for ch in text[lo:hi]:
        if ch == "\n":
            line, col = line + 1, 0
            continue
        lines[line][col] = mark
        col += 1


def _cfg_test_items(text: str) -> list[tuple[int, int]]:
    """The character spans of every item `#[cfg(test)]` marks but a `mod {}`,
    which `test_blocks` reads: a helper function, an `impl`, a `use`."""
    spans = []
    at = 0
    while (at := text.find("#[cfg(test)]", at)) >= 0:
        rest = at + len("#[cfg(test)]")
        brace, semi = text.find("{", rest), text.find(";", rest)
        if re.match(r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\b", text[rest:]):
            at = rest
            continue
        if semi >= 0 and (brace < 0 or semi < brace):
            spans.append((at, semi + 1))
        elif brace >= 0:
            spans.append((at, _close_brace(text, brace) + 1))
        at = rest
    return spans


@dataclass
class Source:
    """One production file: its module, and which characters are code."""

    package: str
    module: str
    rel: str  # repo-relative
    lines: list[str]  # as written
    untested: list[str]  # masked, test code `TEST`
    production: list[str]  # masked, test code blanked
    code: list[str]  # production with `use` declarations blanked too
    impls: list[tuple[Position, Position, str]] = field(default_factory=list)  # char columns

    def char_col(self, pos: Position) -> int:
        line = self.lines[pos[0]] if pos[0] < len(self.lines) else ""
        return len(line.encode()[: pos[1]].decode("utf-8", errors="ignore"))

    def _at(self, grid: list[str], pos: Position) -> bool:
        if pos[0] >= len(grid):
            return False
        col = self.char_col(pos)
        return col < len(grid[pos[0]]) and not grid[pos[0]][col].isspace()

    def is_code(self, pos: Position) -> bool:
        return self._at(self.code, pos)

    def is_production(self, pos: Position) -> bool:
        return self._at(self.production, pos)


def _strip_generics(header: str) -> str:
    """An `impl` header without its leading `<...>` parameter list."""
    header = header.lstrip()
    if not header.startswith("<"):
        return header
    depth = 0
    for k, ch in enumerate(header):
        depth += {"<": 1, ">": -1}.get(ch, 0)
        if depth == 0:
            return header[k + 1 :]
    return header


def read_source(package: str, module: str, rel: str, text: str) -> Source:
    m = masked(text)
    grid = [list(line) for line in m.split("\n")]
    ranges, _ = test_blocks(m)
    for lo, hi in ranges:
        for k in range(lo, hi + 1):
            grid[k] = [TEST] * len(grid[k])
    for lo, hi in _cfg_test_items(m):
        _blank_span(grid, m, lo, hi, TEST)
    untested = ["".join(row) for row in grid]
    production = [row.replace(TEST, " ") for row in untested]
    grid = [list(row) for row in production]
    for u in USE_RE.finditer(m):
        _blank_span(grid, m, u.start(), u.end())
    code = ["".join(row) for row in grid]
    src = Source(package, module, rel, text.split("\n"), untested, production, code)
    for im in IMPL_RE.finditer(m):
        brace = m.find("{", im.end())
        if brace < 0:
            continue
        header = _strip_generics(re.sub(r"\s+", " ", m[im.end() : brace]).split(" where ")[0])
        self_ty = re.split(r"\bfor\b(?!\s*<)", header)[-1]
        self_ty = re.sub(r"<.*", "", self_ty).strip(" &'")
        name = re.findall(r"[A-Za-z_]\w*", self_ty)
        if not name:
            continue
        end = _close_brace(m, brace)

        def pos(k: int) -> Position:
            return m.count("\n", 0, k), k - (m.rfind("\n", 0, k) + 1)

        src.impls.append((pos(im.start()), pos(end), name[-1]))
    return src


def module_name(package: str, src: Path, file: Path) -> str:
    rel = file.relative_to(src).with_suffix("")
    parts = list(rel.parts)
    if parts and parts[-1] in ("lib", "main", "mod"):
        parts = parts[:-1]
    return "::".join([package, *parts])


def workspace_sources(repo: Path, meta: dict) -> dict[str, Source]:
    """Every member's production files, by repo-relative path."""
    out: dict[str, Source] = {}
    ids = set(meta["workspace_members"])
    for package in meta["packages"]:
        if package["id"] not in ids:
            continue
        root = next(
            (Path(t["src_path"]) for t in package["targets"] if {"lib", "bin"} & set(t["kind"])),
            None,
        )
        if root is None:
            continue
        src = root.parent
        for file in layering.module_tree(src, root.name):
            rel = file.relative_to(repo).as_posix()
            text = file.read_text(encoding="utf-8")
            out[rel] = read_source(package["name"], module_name(package["name"], src, file), rel, text)
    return out


# ---------------------------------------------------------------------------
# References.


@dataclass(frozen=True)
class Ref:
    """One reference: from an item in one module to an item in another, or
    the same module (which a design's moves may part)."""

    src_module: str
    src_item: str
    dst_module: str
    dst_item: str
    rel: str
    line: int  # 1-based
    src_member: str | None = None  # the method of `src_item` it sits in, if any
    dst_member: str | None = None  # the method of `dst_item` it names, if any


def _items(src: Source, doc: Document) -> list[tuple[Position, Position, str]]:
    """The file's top-level items as character spans: its definitions'
    enclosing ranges not inside another, and each `impl` block."""
    spans = []
    for o in doc.occurrences:
        if not o.definition or o.enclosing is None or is_module(o.symbol):
            continue
        name = item_name(o.symbol)
        if name is None:
            continue
        (l0, c0), (l1, c1) = o.enclosing
        spans.append(((l0, src.char_col((l0, c0))), (l1, src.char_col((l1, c1))), name))
    spans += src.impls
    return [
        s
        for s in spans
        if not any(t is not s and t[0] <= s[0] and s[1] <= t[1] and t[:2] != s[:2] for t in spans)
    ]


def member_name(symbol: str) -> str | None:
    """The method an `impl`'s symbol belongs to — `eval` in
    `impl#[ResolvedTerm]eval().` or `impl#[Ay][Clone]clone().` — or None."""
    d = descriptors(symbol)
    m = d and re.search(r"impl#\[(?:`[^`]*`|[^\]])*\](?:\[(?:`[^`]*`|[^\]])*\])?(`[^`]*`|[A-Za-z_]\w*)", d)
    return m.group(1).strip("`") if m else None


def _members(src: Source, doc: Document) -> list[tuple[Position, Position, str]]:
    """Each method an `impl` defines, as a character span."""
    spans = []
    for o in doc.occurrences:
        if not o.definition or o.enclosing is None:
            continue
        name = member_name(o.symbol)
        if name is None:
            continue
        (l0, c0), (l1, c1) = o.enclosing
        spans.append(((l0, src.char_col((l0, c0))), (l1, src.char_col((l1, c1))), name))
    return spans


def _member_at(members: list[tuple[Position, Position, str]], pos: Position) -> str | None:
    found = _item_at(members, pos)
    return None if found == TOP else found


def _item_at(items: list[tuple[Position, Position, str]], pos: Position) -> str:
    best = None
    for lo, hi, name in items:
        if lo <= pos <= hi and (best is None or lo < best[0]):
            best = (lo, name)
    return best[1] if best else TOP


@dataclass
class Graph:
    refs: list[Ref]
    items: dict[str, set[str]]  # module -> the items its file defines
    modules: set[str]
    docs: list[Ref]  # doc links, `dst_item` the path as written
    members: dict[tuple[str, str], set[str]] = field(default_factory=dict)  # an item's methods


def read_graph(sources: dict[str, Source], docs: list[Document]) -> Graph:
    defs: dict[str, tuple[str, str, str | None]] = {}
    items: dict[str, set[str]] = {s.module: set() for s in sources.values()}
    members: dict[tuple[str, str], set[str]] = defaultdict(set)
    by_path = {d.path: d for d in docs if d.path in sources}
    spans, methods = {}, {}
    for path, doc in by_path.items():
        src = sources[path]
        spans[path] = _items(src, doc)
        methods[path] = _members(src, doc)
        items[src.module] |= {name for _, _, name in spans[path]}
        for lo, _, name in methods[path]:
            members[(src.module, _item_at(spans[path], lo))].add(name)
        for o in doc.occurrences:
            if o.definition and not is_module(o.symbol) and src.is_production(o.start):
                # The top-level item holding the definition, which a move can
                # name: a generic `impl`'s self type without its parameters, a
                # function for what its body defines.
                name = item_name(o.symbol)
                if name is None:
                    continue
                at = (o.start[0], src.char_col(o.start))
                held = _item_at(spans[path], at)
                defs.setdefault(
                    o.symbol, (src.module, name if held == TOP else held, _member_at(methods[path], at))
                )
    refs: list[Ref] = []
    for path, doc in by_path.items():
        src = sources[path]
        for o in doc.occurrences:
            if o.definition or o.symbol not in defs or not src.is_code(o.start):
                continue
            at = (o.start[0], src.char_col(o.start))
            here, method = _item_at(spans[path], at), _member_at(methods[path], at)
            module, name, member = defs[o.symbol]
            if (module, name, member) != (src.module, here, method):
                refs.append(Ref(src.module, here, module, name, path, o.start[0] + 1, method, member))
    modules = {s.module for s in sources.values()}
    return Graph(refs, items, modules, doc_links(sources, modules), dict(members))


def doc_links(sources: dict[str, Source], modules: set[str]) -> list[Ref]:
    """Every path a production comment names that resolves to another module."""
    packages = {s.package.replace("-", "_"): s.package for s in sources.values()}
    out = []
    for src in sources.values():
        for n, line in enumerate(src.lines):
            for m in DOC_PATH_RE.finditer(line):
                if not _in_comment(src, n, m.start()):
                    continue
                head, segs = m.group(1), m.group(2).strip(":").split("::")
                if head == "crate":
                    base = [src.package]
                elif head == "self":
                    base = src.module.split("::")
                elif head == "super":
                    base = src.module.split("::")[:-1]
                    while segs and segs[0] == "super":
                        base, segs = base[:-1], segs[1:]
                elif head in packages:
                    base = [packages[head]]
                else:
                    continue
                if not base:
                    continue
                target = None
                for k in range(len(segs), -1, -1):
                    cand = "::".join(base + segs[:k])
                    if cand in modules:
                        target = cand
                        break
                if target and target != src.module:
                    out.append(Ref(src.module, TOP, target, m.group(0), src.rel, n + 1))
    return out


def _in_comment(src: Source, n: int, col: int) -> bool:
    """Whether a character is in a production comment: blank once masked,
    and neither test code nor a string."""
    row = src.untested[n] if n < len(src.untested) else ""
    return col < len(row) and row[col] == " " and not src.lines[n][col].isspace()


# ---------------------------------------------------------------------------
# Moves, and the graph they would make.

MOVES_RE = re.compile(r"(?ms)^```moves[^\n]*\n(.*?)^```")


@dataclass
class Moves:
    modules: dict[str, str] = field(default_factory=dict)
    items: dict[tuple[str, str], str] = field(default_factory=dict)
    members: dict[tuple[str, str, str], str] = field(default_factory=dict)

    def target(self, module: str, item: str, member: str | None = None) -> str:
        return (
            (member and self.members.get((module, item, member)))
            or self.items.get((module, item))
            or self.modules.get(module)
            or module
        )


def read_moves(text: str, graph: Graph) -> tuple[Moves, list[str]]:
    """The moves in a design doc's ```` ```moves ```` blocks, and the lines
    naming neither a module, one of its items, nor an item's method."""
    moves, problems = Moves(), []
    for block in MOVES_RE.finditer(text):
        for raw in block.group(1).splitlines():
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if "->" not in line:
                problems.append(f"move `{line}` has no `->`")
                continue
            frm, to = (s.strip() for s in line.split("->", 1))
            if frm in graph.modules:
                moves.modules[frm] = to
                continue
            module, _, item = frm.rpartition("::")
            if module in graph.modules and item in graph.items.get(module, set()):
                moves.items[(module, item)] = to
                continue
            module, _, owner = module.rpartition("::")
            if item in graph.members.get((module, owner), set()):
                moves.members[(module, owner, item)] = to
            else:
                problems.append(
                    f"move `{frm}` names neither a module, an item one defines, nor a method of one"
                )
    return moves, problems


def edges(refs: list[Ref], moves: Moves | None = None) -> dict[tuple[str, str], list[Ref]]:
    out: dict[tuple[str, str], list[Ref]] = defaultdict(list)
    for r in refs:
        a = moves.target(r.src_module, r.src_item, r.src_member) if moves else r.src_module
        b = moves.target(r.dst_module, r.dst_item, r.dst_member) if moves else r.dst_module
        if a != b:
            out[(a, b)].append(r)
    return out


def cycles(nodes: set[str], arcs: set[tuple[str, str]]) -> list[list[str]]:
    """The strongly connected sets of more than one node (Tarjan's, iterative)."""
    succ: dict[str, list[str]] = defaultdict(list)
    for a, b in sorted(arcs):
        succ[a].append(b)
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    stack: list[str] = []
    on: set[str] = set()
    found: list[list[str]] = []
    counter = 0
    for root in sorted(nodes | {n for arc in arcs for n in arc}):
        if root in index:
            continue
        work = [(root, 0)]
        while work:
            node, i = work.pop()
            if i == 0:
                index[node] = low[node] = counter
                counter += 1
                stack.append(node)
                on.add(node)
            children = succ[node]
            if i < len(children):
                work.append((node, i + 1))
                child = children[i]
                if child not in index:
                    work.append((child, 0))
                elif child in on:
                    low[node] = min(low[node], index[child])
                continue
            if work:
                parent = work[-1][0]
                low[parent] = min(low[parent], low[node])
            if low[node] == index[node]:
                group = []
                while True:
                    w = stack.pop()
                    on.discard(w)
                    group.append(w)
                    if w == node:
                        break
                if len(group) > 1:
                    found.append(sorted(group))
    return sorted(found)


def _reach(arcs: set[tuple[str, str]]) -> dict[str, set[str]]:
    succ: dict[str, set[str]] = defaultdict(set)
    for a, b in arcs:
        succ[a].add(b)
    memo: dict[str, set[str]] = {}
    for start in list(succ):
        seen, todo = set(), [start]
        while todo:
            for nxt in succ[todo.pop()]:
                if nxt not in seen:
                    seen.add(nxt)
                    todo.append(nxt)
        memo[start] = seen
    return memo


def _crate(module: str) -> str:
    return module.split("::", 1)[0]


# ---------------------------------------------------------------------------
# The report.


def report(
    graph: Graph,
    out: TextIO,
    moves: Moves | None = None,
    show_docs: bool = False,
    accepted: dict[frozenset[str], str] | None = None,
    owed: dict[frozenset[str], str] | None = None,
) -> int:
    accepted = ACCEPTED if accepted is None else accepted
    owed = OWED if owed is None else owed
    by_arc = edges(graph.refs, moves)
    arcs = set(by_arc)
    nodes = set(graph.modules)
    if moves:
        nodes |= set(moves.modules.values()) | set(moves.items.values())
    found = cycles(nodes, arcs)
    problems = 0
    kept = {"accepted": 0, "owed": 0}
    for group in found:
        members = set(group)
        key = frozenset(group)
        for word, ledger in (("accepted", accepted), ("owed", owed)):
            if key in ledger:
                kept[word] += 1
                print(f"== cycle {word}: {', '.join(group)}: {ledger[key]}", file=out)
                break
        else:
            key = None
        if key is not None:
            continue
        problems += 1
        print(f"!! cycle: {', '.join(group)}", file=out)
        for (a, b), refs in sorted(by_arc.items()):
            if a in members and b in members:
                print(f"     {a} -> {b}", file=out)
                for r in refs[:WITNESSES]:
                    print(f"       {r.rel}:{r.line} {r.src_item} -> {r.dst_item}", file=out)
                if len(refs) > WITNESSES:
                    print(f"       ... {len(refs) - WITNESSES} more", file=out)
    crate_arcs = {(_crate(a), _crate(b)) for a, b in arcs if _crate(a) != _crate(b)}
    for group in cycles(set(), crate_arcs):
        problems += 1
        print(f"!! crate cycle: {', '.join(group)}", file=out)
    if not moves:
        live = {frozenset(g) for g in found}
        for word, ledger in (("acceptance", accepted), ("debt", owed)):
            for key in sorted(ledger, key=sorted):
                if key not in live:
                    problems += 1
                    print(f"!! stale {word}: {', '.join(sorted(key))} is no cycle", file=out)
    reach = _reach(arcs)
    def placed(module: str) -> str:
        return moves.target(module, TOP) if moves else module

    against = [d for d in graph.docs if placed(d.src_module) in reach.get(placed(d.dst_module), set())]
    if show_docs:
        for d in sorted(against, key=lambda d: (d.rel, d.line)):
            print(f"-- doc link against the edges: {d.rel}:{d.line} `{d.dst_item}`", file=out)
    verdict = (
        "no cycle"
        if not found
        else f"{len(found)} cycles, {kept['accepted']} accepted, {kept['owed']} owed"
    )
    what = "the moved tree" if moves else "the tree"
    print(
        f"module-graph: {what}: {len(nodes)} modules, {len(arcs)} edges, {verdict}; "
        f"{len(against)} doc links against the edges"
        + ("" if show_docs else " (--docs lists them)"),
        file=out,
    )
    return 1 if problems else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--design", type=Path, help="a doc whose ```moves blocks to apply")
    parser.add_argument("--docs", action="store_true", help="list doc links against the edges")
    parser.add_argument("--index", type=Path, help="read this SCIP index instead of building one")
    parser.add_argument("--save-index", type=Path, help="keep the index built at this path")
    args = parser.parse_args(argv)
    meta = layering.metadata(REPO)
    sources = workspace_sources(REPO, meta)
    if args.index:
        data = args.index.read_bytes()
    else:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "index.scip"
            try:
                build_index(REPO, path)
            except (RuntimeError, subprocess.CalledProcessError) as err:
                print(f"!! {err}", file=sys.stderr)
                print("module-graph: no index", file=sys.stdout)
                return 2
            data = path.read_bytes()
    if args.save_index:
        args.save_index.write_bytes(data)
    graph = read_graph(sources, read_index(data))
    moves = None
    if args.design:
        moves, problems = read_moves(args.design.read_text(encoding="utf-8"), graph)
        for p in problems:
            print(f"!! {p}")
        if problems:
            print(f"module-graph: {len(problems)} moves unreadable")
            return 1
    return report(graph, sys.stdout, moves, args.docs)


if __name__ == "__main__":
    sys.exit(main())
