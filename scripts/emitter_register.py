#!/usr/bin/env python3
"""The emitter register: every literal `pg_dump` can append to a dump, and
every option it takes, joined against the fixtures that hold them.

`docs/design/roadmap-P31-correctness-evidence.md`, "The emitter register", is
the description; this module is the extraction and the join.

**What it exists to check.** The other reconciliations enumerate *our* side --
the comparison register's arms (`oracle_register.py`), the mapping's rows
(`floor_mapping.py`) -- or the driver's. This one enumerates the producer's: a
form `pg_dump` can write that no fixture holds is a reader nobody has tested
against it, and that class is read out of `pg_dump`'s own source rather than
remembered.

**Two halves, two modes.**

* **The DDL half: a literal.** Every string constant passed to a buffer-append
  call ([`APPEND_CALLS`]) inside the hand-listed functions of [`FUNCTIONS`] --
  the format string, and any constant among its arguments, which is where a
  ternary's branches are (`"UNLOGGED "` is one). A format string contributes
  its longest constant run, a conversion and a format macro between two
  literals (`INT64_FORMAT`) each ending a run, `%%` being a `%` inside one. A
  run under [`MIN_RUN`] characters is no entry, for a plain constant as for a
  format string: what it would assert is punctuation every fixture holds.
* **What a function sends the server is not an emitter.** A buffer whose
  `data` reaches an execute call ([`EXECUTE_CALLS`]) inside the same function
  is a query buffer, and nothing appended to it is a row: a catalog query's
  text is never in a dump. So `setup_connection`, every statement of which
  goes to the server, contributes no row at any major; the settings it pins
  are I4's. A query buffer whose `data` is read anywhere but an execute call
  is a problem the extraction stops on, since its output would be dropped.
* **The option half.** Every entry of `pg_dump`'s and `pg_dumpall`'s
  `long_options` table, with its short letter where it has one. An option is
  covered when a flag set `generate_fixtures.py` runs at that major passes it,
  or when every run passes it ([`generate_fixtures.PG_DUMP_ARGS`],
  `PG_DUMPALL_ARGS`).

**Extraction is generation; the join is the check.** The upstream checkouts
are machine-local, so `--extract` reads them and writes
`fixtures/<major>/emitters.tsv`, and the join reads only committed files: it
runs anywhere the scripts' tests do. The checkout is
`$PGDT_POSTGRES_SOURCE/release-v<minor>`, the minor being the one
`generate_fixtures.ROUTINE_VERSIONS` pins the fixtures at, and its
`configure` must say it is that release -- a register read from a different
minor than the fixtures were taken at would join two different producers.

**A literal is covered when a fixture of the same major holds its bytes**:
any `fixtures/<major>/<schema>/*.sql` of a schema the generator dumps. No C
parser: a branch is reached when what it appends is in some fixture.

**The join reports and does not gate.** It exits non-zero on a problem -- a
major with no register, a malformed row, a flag naming no option -- and lists
what is uncovered without failing on it; dispositions (an `I<n>` or a
`KD<k>` exempting a row) and the gate arrive together.

Usage:

    cd scripts
    uv run emitter_register.py                 # the join, over committed files
    uv run emitter_register.py --extract [--major 18]
    uv run python -m unittest test_emitter_register
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, Sequence, TextIO

import generate_fixtures as gf

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "fixtures"
REGISTER_NAME = "emitters.tsv"

#: Where `release-v<minor>` checkouts live; `CLAUDE.local.md` names this
#: machine's, and the variable moves it.
SOURCE_ENV = "PGDT_POSTGRES_SOURCE"
DEFAULT_SOURCE = Path("/mnt/wd12t/upstream/postgres")

#: The functions whose output some pgdt reader consumes, by file. A function
#: missing from a major's tree is a problem, never a shorter register.
FUNCTIONS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "src/bin/pg_dump/pg_dump.c",
        (
            "dumpTableSchema",
            "dumpCompositeType",
            "dumpEnumType",
            "dumpRangeType",
            "dumpDomain",
            "dumpBaseType",
            "setup_connection",
        ),
    ),
    ("src/bin/pg_dump/pg_backup_archiver.c", ("_printTocEntry",)),
    ("src/fe_utils/string_utils.c", ("appendPsqlMetaConnect",)),
    (
        "src/bin/pg_dump/pg_dumpall.c",
        ("dumpTablespaces", "dropTablespaces", "dumpDatabases", "dropDBs"),
    ),
)

#: Each program's option table, read out of the file holding its `main`.
OPTION_TABLES: tuple[tuple[str, str], ...] = (
    ("src/bin/pg_dump/pg_dump.c", "pg_dump"),
    ("src/bin/pg_dump/pg_dumpall.c", "pg_dumpall"),
)

#: Calls that append to output, and the argument index of the format string
#: for those taking one. `appendPQExpBufferChar` appends one character, under
#: any run's minimum, so it is not listed.
APPEND_CALLS: dict[str, int | None] = {
    "appendPQExpBuffer": 1,
    "printfPQExpBuffer": 1,
    "appendPQExpBufferStr": None,
    "ahprintf": 1,
    "fprintf": 1,
}

#: Calls that send a buffer's text to the server. A buffer whose `data` one of
#: these reads is a query buffer.
EXECUTE_CALLS = frozenset(
    {
        "ExecuteSqlQuery",
        "ExecuteSqlQueryForSingleRow",
        "ExecuteSqlStatement",
        "executeQuery",
        "executeCommand",
        "PQexec",
    }
)

#: Calls reading a query's result. A constant among their arguments names a
#: result column (`PQfnumber(res, "rngmultitype")`), and is no output.
RESULT_CALLS = frozenset({"PQfnumber", "PQgetvalue", "PQgetisnull"})

#: The shortest run that is an entry.
MIN_RUN = 3

#: A printf conversion. `%%` is matched so it can be put back as a `%`.
_CONVERSION = re.compile(
    r"%(?:\d+\$)?[-+ #0']*(?:\d+|\*)?(?:\.(?:\d+|\*))?(?:hh|h|ll|l|L|z|j|t|q)?"
    r"[diouxXeEfFgGaAcspnm%]"
)
#: A macro between two literals is a format piece (`INT64_FORMAT`, `PRId64`).
_FORMAT_MACRO = re.compile(r"^(?:[A-Z][A-Z0-9_]*|PRI[a-zA-Z0-9]+)$")
#: Where a format macro stood between two literals; a row's detail shows it
#: as a bare `%`.
BREAK = "\x00"


# --------------------------------------------------------------------------
# Lexing C
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Token:
    kind: str  # "str", "char", "ident", "num", "punct"
    text: str


_ESCAPES = {
    "n": "\n",
    "t": "\t",
    "r": "\r",
    "\\": "\\",
    '"': '"',
    "'": "'",
    "?": "?",
    "a": "\a",
    "b": "\b",
    "f": "\f",
    "v": "\v",
}


def _unescape(body: str) -> str:
    """A C literal's body as the bytes it denotes (Latin-1 for an escaped
    byte past ASCII, which no listed function writes)."""
    out: list[str] = []
    i = 0
    while i < len(body):
        c = body[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = body[i + 1]
        if nxt in _ESCAPES:
            out.append(_ESCAPES[nxt])
            i += 2
        elif nxt in "01234567":
            j = i + 1
            while j < len(body) and j < i + 4 and body[j] in "01234567":
                j += 1
            out.append(chr(int(body[i + 1 : j], 8)))
            i = j
        elif nxt == "x":
            j = i + 2
            while j < len(body) and body[j] in "0123456789abcdefABCDEF":
                j += 1
            out.append(chr(int(body[i + 2 : j], 16)))
            i = j
        else:
            raise ValueError(f"unknown escape \\{nxt}")
    return "".join(out)


_PUNCT3 = ("...", "<<=", ">>=")
_PUNCT2 = ("->", "++", "--", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>",
           "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "##")


def lex(src: str) -> list[Token]:
    """C tokens, comments and preprocessor lines dropped, literals decoded.

    A comment marker inside a literal is the literal's (`"/* dummy */"` is
    one), which is why this is a lexer rather than a regex."""
    tokens: list[Token] = []
    i = 0
    n = len(src)
    at_line_start = True
    while i < n:
        c = src[i]
        if c == "\n":
            at_line_start = True
            i += 1
            continue
        if c in " \t\r\f\v":
            i += 1
            continue
        if at_line_start and c == "#":
            # A directive runs to an unescaped newline.
            while i < n and src[i] != "\n":
                i += 2 if src[i] == "\\" else 1
            continue
        at_line_start = False
        if src.startswith("//", i):
            while i < n and src[i] != "\n":
                i += 1
            continue
        if src.startswith("/*", i):
            end = src.find("*/", i + 2)
            if end < 0:
                raise ValueError("unterminated comment")
            i = end + 2
            continue
        if c in "\"'":
            j = i + 1
            while j < n and src[j] != c:
                if src[j] == "\n":
                    raise ValueError("newline in a literal")
                j += 2 if src[j] == "\\" else 1
            if j >= n:
                raise ValueError("unterminated literal")
            tokens.append(Token("str" if c == '"' else "char", _unescape(src[i + 1 : j])))
            i = j + 1
            continue
        if c.isalpha() or c == "_":
            j = i
            while j < n and (src[j].isalnum() or src[j] == "_"):
                j += 1
            tokens.append(Token("ident", src[i:j]))
            i = j
            continue
        if c.isdigit():
            j = i
            while j < n and (src[j].isalnum() or src[j] in "._"):
                j += 1
            tokens.append(Token("num", src[i:j]))
            i = j
            continue
        for p in _PUNCT3 + _PUNCT2:
            if src.startswith(p, i):
                tokens.append(Token("punct", p))
                i += len(p)
                break
        else:
            tokens.append(Token("punct", c))
            i += 1
    return tokens


def function_body(text: str, name: str) -> str | None:
    """One C function's body, in PostgreSQL's layout: the name at the start
    of a line, the braces opening and closing at the start of theirs."""
    match = re.search(rf"^{re.escape(name)}\(", text, re.MULTILINE)
    if not match:
        return None
    start = re.compile(r"^\{", re.MULTILINE).search(text, match.end())
    if not start:
        return None
    end = re.compile(r"^\}", re.MULTILINE).search(text, start.end())
    if not end:
        return None
    return text[start.start() : end.end()]


# --------------------------------------------------------------------------
# Calls and their constants
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Call:
    callee: str
    args: tuple[tuple[Token, ...], ...]


_OPEN = {"(": ")", "[": "]", "{": "}"}


def calls(tokens: Sequence[Token], names: Iterable[str]) -> list[Call]:
    """Every call to one of `names`, its arguments split at top-level commas."""
    wanted = set(names)
    out: list[Call] = []
    for i, tok in enumerate(tokens):
        if tok.kind != "ident" or tok.text not in wanted:
            continue
        if i + 1 >= len(tokens) or tokens[i + 1].text != "(":
            continue
        if i > 0 and tokens[i - 1].text in (".", "->"):
            continue
        depth = 0
        args: list[tuple[Token, ...]] = []
        current: list[Token] = []
        for tok2 in tokens[i + 1 :]:
            if tok2.kind == "punct" and tok2.text in _OPEN:
                depth += 1
                if depth == 1:
                    continue
            elif tok2.kind == "punct" and tok2.text in _OPEN.values():
                depth -= 1
                if depth == 0:
                    args.append(tuple(current))
                    break
            elif tok2.kind == "punct" and tok2.text == "," and depth == 1:
                args.append(tuple(current))
                current = []
                continue
            current.append(tok2)
        out.append(Call(tok.text, tuple(args)))
    return out


def _data_reads(tokens: Sequence[Token]) -> list[str]:
    """Each `q->data` or `q.data` among the tokens, as the buffer's name."""
    return [
        a.text
        for a, b, c in zip(tokens, tokens[1:], tokens[2:])
        if a.kind == "ident" and b.text in ("->", ".") and c.text == "data"
    ]


def query_buffers(tokens: Sequence[Token]) -> tuple[set[str], list[str]]:
    """Buffers whose `data` an execute call reads, and any of them whose
    `data` is read anywhere else too -- a buffer that is both a query and
    output, whose output this rule would drop silently."""
    found: set[str] = set()
    executed = 0
    for call in calls(tokens, EXECUTE_CALLS):
        for arg in call.args:
            reads = _data_reads(arg)
            found.update(reads)
            executed += len(reads)
    every = [name for name in _data_reads(tokens) if name in found]
    mixed = sorted(found) if len(every) != executed else []
    return found, mixed


def _buffer(arg: Sequence[Token]) -> str | None:
    names = [t.text for t in arg if t.kind == "ident"]
    return names[0] if len(names) == 1 else None


def constants(arg: Sequence[Token]) -> list[str]:
    """The string constants in one argument, adjacent literals concatenated
    and a macro between two of them standing as a [`BREAK`]. A constant
    inside a [`RESULT_CALLS`] call is none."""
    arg = _without_result_calls(arg)
    out: list[str] = []
    i = 0
    while i < len(arg):
        if arg[i].kind != "str":
            i += 1
            continue
        pieces = [arg[i].text]
        i += 1
        while i < len(arg):
            if arg[i].kind == "str":
                pieces.append(arg[i].text)
                i += 1
            elif (
                arg[i].kind == "ident"
                and _FORMAT_MACRO.match(arg[i].text)
                and i + 1 < len(arg)
                and arg[i + 1].kind == "str"
            ):
                pieces.append(BREAK)
                i += 1
            else:
                break
        out.append("".join(pieces))
    return out


def _without_result_calls(arg: Sequence[Token]) -> list[Token]:
    out: list[Token] = []
    i = 0
    while i < len(arg):
        if (
            arg[i].kind == "ident"
            and arg[i].text in RESULT_CALLS
            and i + 1 < len(arg)
            and arg[i + 1].text == "("
        ):
            depth = 0
            i += 1
            while i < len(arg):
                if arg[i].text in _OPEN:
                    depth += 1
                elif arg[i].text in _OPEN.values():
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            i += 1
            continue
        out.append(arg[i])
        i += 1
    return out


def runs(constant: str, is_format: bool) -> list[str]:
    """A constant's constant runs: itself, or a format string's pieces
    between conversions."""
    if not is_format:
        return [r for r in constant.split(BREAK)]
    pieces: list[str] = []
    for part in constant.split(BREAK):
        current = ""
        last = 0
        for m in _CONVERSION.finditer(part):
            current += part[last : m.start()]
            if m.group(0) == "%%":
                current += "%"
            else:
                pieces.append(current)
                current = ""
            last = m.end()
        current += part[last:]
        pieces.append(current)
    return pieces


def longest_run(constant: str, is_format: bool) -> str | None:
    """The run that is this constant's entry, or `None` under [`MIN_RUN`].
    The first of equal lengths wins, so a re-read is stable."""
    best = max(runs(constant, is_format), key=len)
    return best if len(best) >= MIN_RUN else None


@dataclass(frozen=True)
class Row:
    """One register row: a literal, or an option."""

    kind: str  # "literal" or "option"
    file: str
    function: str  # the C function, or the program, for an option
    entry: str  # the run, or the long option's name
    detail: str  # the whole constant, or the option's short letter

    def tsv(self) -> str:
        return "\t".join(
            (self.kind, self.file, self.function, escape(self.entry), escape(self.detail))
        )


def literals(body: str, file: str, function: str) -> tuple[list[Row], list[str]]:
    """A function's literal rows, one per distinct entry, in first-seen
    order, and the problem a query buffer read as output too would be."""
    tokens = lex(body)
    skip, mixed = query_buffers(tokens)
    problems = [
        f"{file}: `{function}` reads a query buffer's text outside an execute call "
        f"({', '.join(mixed)}) — its output would be dropped as a query's"
    ] if mixed else []
    rows: dict[str, Row] = {}
    for call in calls(tokens, APPEND_CALLS):
        if call.callee.endswith("PQExpBuffer") or call.callee.endswith("PQExpBufferStr"):
            if not call.args or _buffer(call.args[0]) in skip:
                continue
        elif call.args and _buffer(call.args[0]) == "stderr":
            continue
        fmt_index = APPEND_CALLS[call.callee]
        for index, arg in enumerate(call.args):
            for constant in constants(arg):
                is_format = fmt_index is not None and index == fmt_index
                entry = longest_run(constant, is_format)
                if entry is not None and entry not in rows:
                    detail = constant.replace(BREAK, "%" if is_format else "")
                    rows[entry] = Row("literal", file, function, entry, detail)
    return list(rows.values()), problems


def options(text: str, file: str, program: str) -> tuple[list[Row], list[str]]:
    """A program's `long_options` table, one row per entry."""
    match = re.search(r"\blong_options\[\]\s*=\s*\{", text)
    if not match:
        return [], [f"{file}: no `long_options[]` table"]
    tokens = lex(text[match.end() - 1 :])
    depth = 0
    entry: list[Token] = []
    rows: list[Row] = []
    for tok in tokens:
        if tok.text == "{":
            depth += 1
            if depth == 2:
                entry = []
            continue
        if tok.text == "}":
            depth -= 1
            if depth == 1 and entry and entry[0].kind == "str":
                short = ""
                fields = [list(g) for g in _split(entry, ",")]
                if len(fields) == 4 and len(fields[3]) == 1 and fields[3][0].kind == "char":
                    short = fields[3][0].text
                rows.append(Row("option", file, program, entry[0].text, short))
            if depth == 0:
                break
            continue
        if depth == 2:
            entry.append(tok)
    if not rows:
        return [], [f"{file}: `long_options[]` holds no entries"]
    return rows, []


def _split(tokens: Sequence[Token], sep: str) -> Iterable[Sequence[Token]]:
    current: list[Token] = []
    for tok in tokens:
        if tok.text == sep:
            yield current
            current = []
        else:
            current.append(tok)
    yield current


# --------------------------------------------------------------------------
# The register file
# --------------------------------------------------------------------------

_ESCAPE = {"\\": "\\\\", "\n": "\\n", "\t": "\\t", "\r": "\\r"}
_UNESCAPE = {"\\": "\\", "n": "\n", "t": "\t", "r": "\r"}
HEADER = ("kind", "file", "function", "entry", "detail")


def escape(s: str) -> str:
    return "".join(_ESCAPE.get(c, c) for c in s)


def unescape(s: str) -> str:
    out: list[str] = []
    i = 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s) and s[i + 1] in _UNESCAPE:
            out.append(_UNESCAPE[s[i + 1]])
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


#: The first line of a register, naming the release it was read from.
_RELEASE_LINE = re.compile(r"^# The emitter register of PostgreSQL (\S+), ")


def render(rows: Sequence[Row], release: str) -> str:
    lines = [
        f"# The emitter register of PostgreSQL {release}, written by "
        "`scripts/emitter_register.py --extract`; never edited by hand.",
        "\t".join(HEADER),
    ]
    lines.extend(row.tsv() for row in rows)
    return "\n".join(lines) + "\n"


@dataclass
class Register:
    """A register file, read back."""

    release: str | None = None
    rows: list[Row] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)


def parse(text: str, where: str = "<register>") -> Register:
    out = Register()
    rows, problems = out.rows, out.problems
    header_seen = False
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("#"):
            if m := _RELEASE_LINE.match(line):
                out.release = m.group(1)
            continue
        if not line:
            continue
        fields = line.split("\t")
        if not header_seen:
            if tuple(fields) != HEADER:
                problems.append(f"{where}:{number}: header is not {HEADER}")
            header_seen = True
            continue
        if len(fields) != len(HEADER) or fields[0] not in ("literal", "option"):
            problems.append(f"{where}:{number}: malformed row {line!r}")
            continue
        kind, file, function, entry, detail = fields
        rows.append(Row(kind, file, function, unescape(entry), unescape(detail)))
    if not header_seen:
        problems.append(f"{where}: no header")
    if out.release is None:
        problems.append(f"{where}: names no release")
    return out


# --------------------------------------------------------------------------
# Extraction
# --------------------------------------------------------------------------


def checkout_release(checkout: Path) -> str | None:
    """The release a checkout's `configure` says it is."""
    configure = checkout / "configure"
    if not configure.is_file():
        return None
    m = re.search(r"^PACKAGE_VERSION='([^']+)'", configure.read_text(errors="replace"), re.MULTILINE)
    return m.group(1) if m else None


def pinned_release(major: str) -> str:
    """The minor `generate_fixtures` takes this major's fixtures at."""
    image = gf.ROUTINE_VERSIONS[major]
    return image.split(":", 1)[1].split("-", 1)[0]


def extract(checkout: Path) -> tuple[list[Row], list[str]]:
    """Every row a checkout yields, or the problems that stopped it."""
    rows: list[Row] = []
    problems: list[str] = []
    for file, names in FUNCTIONS:
        path = checkout / file
        if not path.is_file():
            problems.append(f"{path}: missing")
            continue
        text = path.read_text(errors="replace")
        for name in names:
            body = function_body(text, name)
            if body is None:
                problems.append(f"{file}: no function `{name}` — the emitter has moved")
                continue
            found, more = literals(body, file, name)
            rows.extend(found)
            problems.extend(more)
    for file, program in OPTION_TABLES:
        path = checkout / file
        if not path.is_file():
            problems.append(f"{path}: missing")
            continue
        found, more = options(path.read_text(errors="replace"), file, program)
        rows.extend(found)
        problems.extend(more)
    return rows, problems


def source_root() -> Path:
    return Path(os.environ.get(SOURCE_ENV) or DEFAULT_SOURCE)


def extract_major(major: str, root: Path, fixtures: Path = FIXTURES) -> list[str]:
    release = pinned_release(major)
    checkout = root / f"release-v{release}"
    stated = checkout_release(checkout)
    if stated != release:
        return [f"{checkout}: says it is {stated!r}, the fixtures are {release!r}"]
    rows, problems = extract(checkout)
    if problems:
        return problems
    (fixtures / major / REGISTER_NAME).write_text(render(rows, release))
    return []


# --------------------------------------------------------------------------
# The join
# --------------------------------------------------------------------------


def flags_at(major: str) -> dict[str, list[str]]:
    """Every argument some run of each program passes at this major."""
    out: dict[str, list[str]] = {"pg_dump": list(gf.PG_DUMP_ARGS), "pg_dumpall": list(gf.PG_DUMPALL_ARGS)}
    for sets in gf.SCHEMAS.values():
        for value in sets.values():
            if isinstance(value, tuple):
                minimum, flags = value
                if int(major) < int(minimum):
                    continue
            else:
                flags = value
            if flags is not None:
                out["pg_dump"].extend(flags)
    return out


def resolve_flags(argv: Sequence[str], table: Sequence[Row]) -> tuple[set[str], list[str]]:
    """The long names a list of arguments states, by long or short form."""
    by_short = {row.detail: row.entry for row in table if row.detail}
    names = {row.entry for row in table}
    found: set[str] = set()
    problems: list[str] = []
    for arg in argv:
        if arg.startswith("--"):
            name = arg[2:].split("=", 1)[0]
        elif arg.startswith("-") and len(arg) >= 2:
            name = by_short.get(arg[1], "")
        else:
            continue
        if name in names:
            found.add(name)
        else:
            program = table[0].function if table else "a program with no option rows"
            problems.append(f"{arg!r} names no option of {program}")
    return found, problems


def dump_files(major_dir: Path) -> list[Path]:
    """The dumps a literal may resolve to: each schema the generator dumps."""
    return sorted(
        path
        for schema in gf.SCHEMAS
        for path in (major_dir / schema).glob("*.sql")
    )


@dataclass
class MajorResult:
    major: str
    literals: int = 0
    options: int = 0
    uncovered: list[Row] = field(default_factory=list)


def join_major(major: str, fixtures: Path = FIXTURES) -> tuple[MajorResult, list[str]]:
    """One major's rows against its fixtures. A register read from another
    minor than the fixtures were taken at is a problem: it joins two
    producers."""
    result = MajorResult(major)
    path = fixtures / major / REGISTER_NAME
    if not path.is_file():
        return result, [f"{path}: missing — run `emitter_register.py --extract`"]
    register = parse(path.read_text(), str(path))
    rows, problems = register.rows, list(register.problems)
    pinned = pinned_release(major)
    if register.release is not None and register.release != pinned:
        problems.append(
            f"{path}: read from {register.release}, the fixtures are {pinned} — "
            "re-run `emitter_register.py --extract`"
        )
    dumps = [p.read_bytes() for p in dump_files(fixtures / major)]
    if not dumps:
        problems.append(f"{fixtures / major}: no dumps")
    covered_options: dict[str, set[str]] = {}
    for program, argv in flags_at(major).items():
        table = [r for r in rows if r.kind == "option" and r.function == program]
        found, more = resolve_flags(argv, table)
        covered_options[program] = found
        problems.extend(f"{major}: {p}" for p in more)
    for row in rows:
        if row.kind == "literal":
            result.literals += 1
            needle = row.entry.encode("latin-1")
            if not any(needle in dump for dump in dumps):
                result.uncovered.append(row)
        else:
            result.options += 1
            if row.entry not in covered_options.get(row.function, set()):
                result.uncovered.append(row)
    return result, problems


def majors() -> list[str]:
    return sorted(gf.ROUTINE_VERSIONS, key=int)


def report(results: Sequence[MajorResult]) -> str:
    lines = []
    for r in results:
        n_lit = sum(1 for u in r.uncovered if u.kind == "literal")
        n_opt = sum(1 for u in r.uncovered if u.kind == "option")
        lines.append(
            f"{r.major}: {r.literals} literals, {n_lit} uncovered; "
            f"{r.options} options, {n_opt} uncovered"
        )
    where: dict[tuple[str, str, str, str], list[str]] = defaultdict(list)
    for r in results:
        for u in r.uncovered:
            where[(u.kind, u.file.rsplit("/", 1)[-1], u.function, u.entry)].append(r.major)
    lines.append("")
    lines.append("uncovered (majors):")
    for (kind, file, function, entry), at in sorted(where.items()):
        lines.append(f"  {kind:7} {file}:{function} {entry!r}  [{_span(at)}]")
    return "\n".join(lines)


def _span(at: Sequence[str]) -> str:
    nums = sorted(int(a) for a in at)
    every = sorted(int(m) for m in gf.ROUTINE_VERSIONS)
    if nums == every:
        return "all"
    return ",".join(str(n) for n in nums)


def main(argv: Sequence[str] | None = None, out: TextIO = sys.stdout) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--extract",
        action="store_true",
        help=f"read the checkouts under ${SOURCE_ENV} (default {DEFAULT_SOURCE}) "
        f"and write fixtures/<major>/{REGISTER_NAME}",
    )
    parser.add_argument("--major", action="append", help="limit to these majors")
    args = parser.parse_args(argv)
    chosen = args.major or majors()
    if args.extract:
        problems = []
        for major in chosen:
            problems.extend(extract_major(major, source_root()))
        for p in problems:
            print(f"problem: {p}", file=sys.stderr)
        return 1 if problems else 0
    results = []
    problems = []
    for major in chosen:
        result, more = join_major(major)
        results.append(result)
        problems.extend(more)
    print(report(results), file=out)
    for p in problems:
        print(f"problem: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
