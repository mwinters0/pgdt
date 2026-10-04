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
* **What a function sends the server is not an emitter.** An append whose
  buffer only execute calls ([`EXECUTE_CALLS`]) read after it, before the
  buffer is next reset, is a query's text, and no row: a catalog query's text
  is never in a dump ([`query_appends`]). So one buffer used for a query and
  then for output, or executed when connected and printed otherwise, yields
  its output's rows; a diagnostic quoting a buffer is neither, and a buffer
  quoted into a shell command (`pg_dumpall`'s options for `pg_dump`) is no
  output. `setup_connection`, every statement of which goes to the server,
  contributes no row at any major; the settings it pins are I4's.
* **The function list follows the readers.** [`FUNCTIONS`] lists the
  functions whose output some pgdt reader consumes, each with the majors it
  exists at, and [`READS`] maps every keyword the scanner, the `COPY` framing,
  the lexer, the map and the preamble recognise ([`reader_keywords`], read
  out of their Rust source) to the listed functions writing it, or to the
  statement it is a clause of. A keyword with no row, a row no reader holds,
  or a function a row names that the list lacks fails the join
  ([`reads_problems`]), so a reader taught a statement brings its emitter
  into the list.
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

* **The value-form half: a spelling.** What a backend `*_out` function writes
  under a setting `pg_dump` leaves unpinned (I4), or under an option that
  overrides one it pins, is no literal in `pg_dump`'s source, so it is
  hand-listed here ([`VALUE_FORMS`]) rather than extracted: each row the bytes
  a `COPY` block holds for it -- after `COPY`'s own escaping -- and the
  session-setting variant or flag set `generate_fixtures.py` runs to reach it
  (`docs/design/roadmap-P31-correctness-evidence.md`, "The session-setting
  axis"). A row is joined at every major, as a literal is.

**A literal is covered when a fixture of the same major holds its bytes**, and
so is a value form:
any `fixtures/<major>/<schema>/*.sql` of a schema the generator dumps. No C
parser: a branch is reached when what it appends is in some fixture.

**A row no fixture reaches is exempt or it fails the join** -- and so
`mise run check`, through `test_emitter_register`. [`EXEMPTIONS`] says why
each such row need not be reached, resolved against the committed record: an
`I<n>` proving no producer the generator runs writes it, or a `KD<k>` naming
it a known failure -- the only two a literal may take, since every byte
passes through the map -- and, for an option, a line of the program's source
showing it changes no output byte ([`NoOutput`]), or a
`pg-dump-compatibility.md` row whose Status says the input its other values
write is not read yet ([`Unsupported`]), which lapses when that row is. A `NoOutput` needle is found by the
extraction, which no check may repeat, and written into the register as an
`evidence` row, so the join holds the exemption to the source at every major
holding the option. An exemption whose row a fixture reaches, or that no
register holds, is a problem: it is struck. Besides those, the join exits
non-zero on a major with no register, a malformed row or a flag naming no
option.

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

@dataclass(frozen=True)
class Emitter:
    """A listed function, and the majors it exists at: absent before `first`
    and after `last`, and a problem only between them."""

    name: str
    first: str | None = None
    last: str | None = None

    def at(self, major: str) -> bool:
        return (self.first is None or int(major) >= int(self.first)) and (
            self.last is None or int(major) <= int(self.last)
        )


#: The functions whose output some pgdt reader consumes, by file: what
#: [`READS`] names, and nothing it does not. A function missing from a major
#: it should exist at is a problem, never a shorter register.
FUNCTIONS: tuple[tuple[str, tuple[Emitter, ...]], ...] = (
    (
        "src/bin/pg_dump/pg_dump.c",
        (
            Emitter("dumpTableSchema"),
            Emitter("dumpTableAttach", first="14"),
            Emitter("dumpTableData"),
            Emitter("dumpTableData_copy"),
            Emitter("dumpTableData_insert"),
            Emitter("dumpConstraint"),
            Emitter("dumpCompositeType"),
            Emitter("dumpEnumType"),
            Emitter("dumpRangeType"),
            Emitter("dumpDomain"),
            Emitter("dumpBaseType"),
            Emitter("dumpShellType"),
            Emitter("dumpUndefinedType"),
            Emitter("dumpExtension"),
            Emitter("dumpCollation"),
            Emitter("dumpSearchPath"),
            Emitter("setup_connection"),
        ),
    ),
    (
        "src/bin/pg_dump/pg_backup_archiver.c",
        (
            Emitter("RestoreArchive"),
            Emitter("_printTocEntry"),
            Emitter("_doSetFixedOutputState"),
            Emitter("_doSetSessionAuth"),
            Emitter("_selectOutputSchema"),
            Emitter("_selectTablespace"),
            Emitter("_selectTableAccessMethod"),
            Emitter("StartRestoreBlobs", last="15"),
            Emitter("EndRestoreBlobs", last="15"),
            Emitter("StartRestoreLOs", first="16"),
            Emitter("EndRestoreLOs", first="16"),
        ),
    ),
    ("src/bin/pg_dump/dumputils.c", (Emitter("buildACLCommands"), Emitter("buildDefaultACLCommands"))),
    ("src/fe_utils/string_utils.c", (Emitter("appendPsqlMetaConnect"),)),
    (
        "src/bin/pg_dump/pg_dumpall.c",
        (
            Emitter("main"),
            Emitter("dumpRoleMembership"),
            Emitter("dumpTablespaces"),
            Emitter("dropTablespaces"),
            Emitter("dumpDatabases"),
            Emitter("dropDBs"),
        ),
    ),
)


def listed() -> dict[str, Emitter]:
    """Every listed function by name."""
    return {e.name: e for _, emitters in FUNCTIONS for e in emitters}


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
    "archprintf": 1,
    "archputs": None,
    "fprintf": 1,
}

#: The append calls whose first argument is a buffer, rather than a stream.
BUFFER_APPEND_CALLS = frozenset({"appendPQExpBuffer", "printfPQExpBuffer", "appendPQExpBufferStr"})

#: Calls that empty a buffer: what was appended before one is never read
#: after it.
RESET_CALLS = frozenset(
    {"resetPQExpBuffer", "initPQExpBuffer", "termPQExpBuffer", "destroyPQExpBuffer"}
)

#: Calls quoting a word into a shell command: a buffer one appends to is a
#: command line, never output ([`command_buffers`]).
SHELL_CALLS = frozenset({"appendShellString"})

#: Calls that send a buffer's text to the server. An append whose buffer only
#: these read is a query's text ([`query_appends`]).
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

#: Calls that report to the user: a buffer they quote is neither a query nor
#: output (`pg_log_error_detail("Command was: %s", q->data)`), and so is one
#: `fprintf(stderr, …)` quotes.
DIAGNOSTIC_CALLS = frozenset(
    {
        "pg_log_error",
        "pg_log_error_detail",
        "pg_log_error_hint",
        "pg_log_warning",
        "pg_log_warning_detail",
        "pg_log_info",
        "pg_log_debug",
        "pg_fatal",
        "fatal",
        "exit_horribly",
        "warn_or_exit_horribly",
    }
)

#: Calls reading a query's result. A constant among their arguments names a
#: result column (`PQfnumber(res, "rngmultitype")`), and is no output.
RESULT_CALLS = frozenset({"PQfnumber", "PQgetvalue", "PQgetisnull"})

#: Calls a constant is an argument of without being written: a result
#: column's name, and a string measured or compared
#: (`fmtId(grantee->data + strlen("group "))`).
UNWRITTEN_CALLS = RESULT_CALLS | {"strlen", "strcmp", "strncmp"}

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
    #: The index of the callee's token.
    at: int = -1


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
        out.append(Call(tok.text, tuple(args), i))
    return out


def _data_reads(tokens: Sequence[Token]) -> list[str]:
    """Each `q->data` or `q.data` among the tokens, as the buffer's name."""
    return [name for _, name in _data_reads_at(tokens)]


def _data_reads_at(tokens: Sequence[Token]) -> list[tuple[int, str]]:
    """Each `q->data` or `q.data`, with the index of the buffer's token."""
    return [
        (i, a.text)
        for i, (a, b, c) in enumerate(zip(tokens, tokens[1:], tokens[2:]))
        if a.kind == "ident" and b.text in ("->", ".") and c.text == "data"
    ]


def _call_spans(tokens: Sequence[Token], names: Iterable[str]) -> list[tuple[str, int, int]]:
    """Each call to one of `names`: its callee and the token span from the
    callee to its closing parenthesis."""
    wanted = set(names)
    out: list[tuple[str, int, int]] = []
    for i, tok in enumerate(tokens):
        if tok.kind != "ident" or tok.text not in wanted:
            continue
        if i + 1 >= len(tokens) or tokens[i + 1].text != "(":
            continue
        if i > 0 and tokens[i - 1].text in (".", "->"):
            continue
        depth = 0
        for j in range(i + 1, len(tokens)):
            if tokens[j].kind == "punct" and tokens[j].text in _OPEN:
                depth += 1
            elif tokens[j].kind == "punct" and tokens[j].text in _OPEN.values():
                depth -= 1
                if depth == 0:
                    out.append((tok.text, i, j))
                    break
    return out


def query_appends(tokens: Sequence[Token]) -> set[int]:
    """The appends that are a query's text rather than output, by the index
    of their callee's token.

    An append is output when some read of its buffer's `data` after it,
    before the buffer's next reset ([`RESET_CALLS`], or a
    `printfPQExpBuffer`, which resets before it appends), is not inside an
    execute call ([`EXECUTE_CALLS`]); it is a query's when every such read
    is, and there is at least one. A buffer nothing reads here is output: it
    is handed out by pointer, as `appendPsqlMetaConnect`'s is. So one buffer
    used for a catalog query and then for output, or executed when connected
    and printed otherwise, is read in token order, which is source order:
    the branch that prints is a read after the append like the one that
    executes."""
    executed: set[int] = set()
    for _, start, end in _call_spans(tokens, EXECUTE_CALLS):
        executed.update(start + i for i, _ in _data_reads_at(tokens[start : end + 1]))
    diagnostic: set[int] = set()
    for callee, start, end in _call_spans(tokens, DIAGNOSTIC_CALLS | {"fprintf"}):
        if callee == "fprintf" and tokens[start + 2].text != "stderr":
            continue
        diagnostic.update(start + i for i, _ in _data_reads_at(tokens[start : end + 1]))
    events: dict[str, list[tuple[int, str]]] = defaultdict(list)
    for index, name in _data_reads_at(tokens):
        if index in diagnostic or (index + 3 < len(tokens) and tokens[index + 3].text == "["):
            # A diagnostic quoting the buffer, or a byte of it rewritten in
            # place, is neither a query nor output.
            continue
        events[name].append((index, "exec" if index in executed else "read"))
    for call in calls(tokens, RESET_CALLS | {"printfPQExpBuffer"}):
        name = _buffer(call.args[0]) if call.args else None
        if name is not None:
            events[name].append((call.at, "reset"))
    for i, tok in enumerate(tokens):
        # `q = createPQExpBuffer();` starts a buffer afresh.
        if (
            tok.text == "createPQExpBuffer"
            and i >= 2
            and tokens[i - 1].text == "="
            and tokens[i - 2].kind == "ident"
        ):
            events[tokens[i - 2].text].append((i, "reset"))
    query: set[int] = set()
    for call in calls(tokens, BUFFER_APPEND_CALLS):
        name = _buffer(call.args[0]) if call.args else None
        if name is None:
            continue
        reads = []
        for _, kind in sorted(e for e in events.get(name, []) if e[0] > call.at):
            if kind == "reset":
                break
            reads.append(kind)
        if reads and all(kind == "exec" for kind in reads):
            query.add(call.at)
    return query


def command_buffers(tokens: Sequence[Token]) -> set[str]:
    """Buffers holding a command line rather than output: any a
    [`SHELL_CALLS`] call quotes into, as `pg_dumpall` builds the options it
    runs each database's `pg_dump` with."""
    return {
        name
        for call in calls(tokens, SHELL_CALLS)
        if call.args and (name := _buffer(call.args[0])) is not None
    }


def _buffer(arg: Sequence[Token]) -> str | None:
    names = [t.text for t in arg if t.kind == "ident"]
    return names[0] if len(names) == 1 else None


def constants(arg: Sequence[Token]) -> list[str]:
    """The string constants in one argument, adjacent literals concatenated
    and a macro between two of them standing as a [`BREAK`]. A constant
    inside an [`UNWRITTEN_CALLS`] call is none."""
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
            and arg[i].text in UNWRITTEN_CALLS
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
    """One register row: a literal, an option, or the evidence that an
    option exempt as changing no output byte is consumed where its exemption
    says ([`NoOutput`])."""

    kind: str  # "literal", "option" or "evidence"
    file: str
    function: str  # the C function, or the program, for an option
    entry: str  # the run, or the long option's name
    detail: str  # the whole constant, the option's short letter, or the needle

    def tsv(self) -> str:
        return "\t".join(
            (self.kind, self.file, self.function, escape(self.entry), escape(self.detail))
        )


def literals(body: str, file: str, function: str) -> list[Row]:
    """A function's literal rows, one per distinct entry, in first-seen
    order."""
    tokens = lex(body)
    query = query_appends(tokens)
    commands = command_buffers(tokens)
    rows: dict[str, Row] = {}
    for call in calls(tokens, APPEND_CALLS):
        if call.callee in BUFFER_APPEND_CALLS:
            if not call.args or call.at in query or _buffer(call.args[0]) in commands:
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
    return list(rows.values())


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
# The readers' keywords
# --------------------------------------------------------------------------

#: The pgdt modules that read a dump's statements: the scanner, the `COPY`
#: framing, the lexer, the map's classification and the preamble's grammar.
READERS: tuple[str, ...] = (
    "pgdump_query/src/scan.rs",
    "pgdump_query/src/copy.rs",
    "pgdump_query/src/lex.rs",
    "pgdump_query/src/map.rs",
    "pgdump_query/src/preamble.rs",
)

#: Calls a reader matches a keyword with: a constant they take is a keyword
#: whatever its case (`eat_keyword(b"copy")`).
KEYWORD_CALLS = frozenset(
    {
        "eat_keyword",
        "strip_word",
        "strip_kw",
        "find_ci",
        "ident_after",
        "eq_ignore_ascii_case",
        "upper_prefix",
        "holds_word",
        "parenthesized",
    }
)

#: A constant anywhere in a reader that is a keyword by its shape: led by an
#: upper-case SQL word, a psql meta-command, `COPY`'s terminator, or a comment
#: line the reader reads (`-- Name: `).
_KEYWORD_SHAPE = re.compile(r"^(?:\\[a-z]{2,}|\\\.$|-- |[A-Z][A-Z_]+(?![A-Za-z0-9_]))")

_RUST_ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"', "'": "'", "0": "\0"}


def rust_tokens(src: str) -> list[Token]:
    """Rust tokens, comments dropped and string literals decoded (a byte
    string as its text): enough to find each constant and the call it is an
    argument of. A lifetime is no literal."""
    tokens: list[Token] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in " \t\r\n":
            i += 1
            continue
        if src.startswith("//", i):
            end = src.find("\n", i)
            i = n if end < 0 else end
            continue
        if src.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            continue
        raw = re.match(r'b?r(#*)"', src[i:])
        if raw and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            close = '"' + raw.group(1)
            end = src.find(close, i + raw.end())
            tokens.append(Token("str", src[i + raw.end() : end]))
            i = end + len(close)
            continue
        if c == '"' or (c == "b" and src.startswith('"', i + 1) and not (i and (src[i - 1].isalnum() or src[i - 1] == "_"))):
            i += 1 if c == '"' else 2
            out: list[str] = []
            while src[i] != '"':
                if src[i] == "\\":
                    e = src[i + 1]
                    if e == "\n":
                        i += 2
                        while src[i] in " \t\r\n":
                            i += 1
                        continue
                    if e == "x":
                        out.append(chr(int(src[i + 2 : i + 4], 16)))
                        i += 4
                        continue
                    if e == "u":
                        end = src.index("}", i)
                        out.append(chr(int(src[i + 3 : end], 16)))
                        i = end + 1
                        continue
                    out.append(_RUST_ESCAPES[e])
                    i += 2
                    continue
                out.append(src[i])
                i += 1
            tokens.append(Token("str", "".join(out)))
            i += 1
            continue
        char = re.match(r"b?'(?:\\u\{[0-9a-fA-F]+\}|\\x[0-9a-fA-F]{2}|\\.|[^\\'])'", src[i:])
        if char and (c == "'" or not (i and (src[i - 1].isalnum() or src[i - 1] == "_"))):
            tokens.append(Token("char", char.group(0)))
            i += char.end()
            continue
        if c == "'":
            i += 1  # a lifetime
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
        tokens.append(Token("punct", c))
        i += 1
    return tokens


def reader_keywords(src: str) -> list[str]:
    """The keywords one reader recognises, in first-seen order: each
    constant of its non-test code that a [`KEYWORD_CALLS`] call takes or
    whose shape is a keyword's. The test module is the file's last item, so
    everything from its `#[cfg(test)]` on is not read."""
    cut = src.find("#[cfg(test)]")
    tokens = rust_tokens(src if cut < 0 else src[:cut])
    found: dict[str, None] = {}
    opens: list[int] = []
    for i, tok in enumerate(tokens):
        if tok.kind == "punct" and tok.text in "([{":
            opens.append(i)
        elif tok.kind == "punct" and tok.text in ")]}":
            if opens:
                opens.pop()
        elif tok.kind == "str":
            callee = None
            if opens and tokens[opens[-1]].text == "(" and opens[-1] > 0:
                before = tokens[opens[-1] - 1]
                callee = before.text if before.kind == "ident" else None
            if callee in KEYWORD_CALLS or _KEYWORD_SHAPE.match(tok.text):
                found.setdefault(tok.text)
    return list(found)


@dataclass(frozen=True)
class Read:
    """A keyword one reader dispatches on -- what tells it which statement,
    or which form of one, it holds -- and every listed function writing a
    statement it reads there."""

    reader: str
    keyword: str
    emitters: tuple[str, ...]


@dataclass(frozen=True)
class Clause:
    """A keyword one reader reads only inside a statement a [`Read`] of the
    same reader names, so that statement's emitters are this keyword's."""

    reader: str
    keyword: str
    within: tuple[str, ...]


_SCAN, _COPY, _LEX, _MAP, _PREAMBLE = READERS

_TYPE_EMITTERS = (
    "dumpCompositeType",
    "dumpEnumType",
    "dumpRangeType",
    "dumpBaseType",
    "dumpShellType",
    "dumpUndefinedType",
)
_ALTER_TABLE_EMITTERS = ("dumpTableSchema", "dumpTableAttach", "dumpConstraint")


def _clauses(reader: str, within: tuple[str, ...], *keywords: str) -> tuple[Clause, ...]:
    return tuple(Clause(reader, keyword, within) for keyword in keywords)


#: Every keyword a reader recognises ([`reader_keywords`]), each either a
#: [`Read`] naming the emitters of what it reads or a [`Clause`] inside one.
#: A keyword in neither fails the check, and so does a row whose keyword no
#: reader holds, or a `Read` naming a function [`FUNCTIONS`] does not list
#: (docs/design/roadmap-P31-correctness-evidence.md, "The emitter register").
READS: tuple[Read | Clause, ...] = (
    # The scanner: a large-object region, opened and closed by its own
    # transaction (and, under `pg_restore`'s options, by RestoreArchive's).
    Read(_SCAN, "BEGIN;", ("StartRestoreBlobs", "StartRestoreLOs", "RestoreArchive")),
    Read(_SCAN, "COMMIT;", ("EndRestoreBlobs", "EndRestoreLOs", "RestoreArchive")),
    # `COPY` framing: the header and the terminator.
    Read(_COPY, "copy", ("dumpTableData",)),
    *_clauses(_COPY, ("copy",), "from", "stdin"),
    Read(_COPY, "\\.", ("dumpTableData_copy",)),
    # The lexer's literal syntax (I50).
    Read(_LEX, "SET", ("_doSetFixedOutputState", "main")),
    *_clauses(_LEX, ("SET",), "standard_conforming_strings", "TO"),
    # The map: TOC headers, the dump's own header, framing, data runs.
    Read(_MAP, "-- ", ("_printTocEntry",)),
    Read(_MAP, "-- Name: ", ("_printTocEntry",)),
    Read(_MAP, "-- Statistics for Name: ", ("_printTocEntry",)),
    Read(_MAP, "-- load via partition root ", ("dumpTableData",)),
    Read(_MAP, "INSERT INTO ", ("dumpTableData_insert",)),
    Read(_MAP, "-- Dumped from database version ", ("RestoreArchive",)),
    Read(_MAP, "-- Dumped by pg_dump version ", ("RestoreArchive",)),
    Read(
        _MAP,
        "SET ",
        (
            "_doSetFixedOutputState",
            "_doSetSessionAuth",
            "_selectOutputSchema",
            "_selectTablespace",
            "_selectTableAccessMethod",
            "buildACLCommands",
            "main",
        ),
    ),
    Read(_MAP, "SELECT pg_catalog.set_config(", ("dumpSearchPath",)),
    Read(_MAP, "ALTER TYPE", ("dumpEnumType", "dumpCompositeType")),
    # The preamble: each statement it folds, and the forms of each.
    Read(_PREAMBLE, "CREATE TABLE", ("dumpTableSchema",)),
    Read(_PREAMBLE, "CREATE UNLOGGED TABLE", ("dumpTableSchema",)),
    Read(_PREAMBLE, "CREATE FOREIGN TABLE", ("dumpTableSchema",)),
    Read(_PREAMBLE, "OF", ("dumpTableSchema",)),
    Read(_PREAMBLE, "PARTITION", ("dumpTableSchema", "dumpTableAttach")),
    Read(_PREAMBLE, "ALTER TABLE", _ALTER_TABLE_EMITTERS),
    Read(_PREAMBLE, "ALTER FOREIGN TABLE", _ALTER_TABLE_EMITTERS),
    # v13 writes `ATTACH PARTITION` in dumpTableSchema, 14 on in its own.
    Read(_PREAMBLE, "ATTACH", ("dumpTableAttach", "dumpTableSchema")),
    Read(_PREAMBLE, "ADD", ("dumpConstraint", "dumpEnumType")),
    Read(_PREAMBLE, "INHERIT", ("dumpTableSchema",)),
    Read(_PREAMBLE, "ALTER", ("dumpTableSchema",)),
    Read(_PREAMBLE, "SET", ("dumpTableSchema",)),
    Read(_PREAMBLE, "CREATE TYPE", _TYPE_EMITTERS),
    Read(_PREAMBLE, "AS", ("dumpCompositeType", "dumpDomain")),
    Read(_PREAMBLE, "AS ENUM", ("dumpEnumType",)),
    Read(_PREAMBLE, "AS RANGE", ("dumpRangeType",)),
    Read(_PREAMBLE, "VALUE", ("dumpEnumType",)),
    Read(_PREAMBLE, "DROP ATTRIBUTE", ("dumpCompositeType",)),
    Read(_PREAMBLE, "CREATE DOMAIN", ("dumpDomain",)),
    Read(_PREAMBLE, "CREATE EXTENSION", ("dumpExtension",)),
    Read(_PREAMBLE, "CREATE COLLATION", ("dumpCollation",)),
    Read(_PREAMBLE, "OWNER TO ", ("_printTocEntry",)),
    Read(_PREAMBLE, "ALTER DEFAULT PRIVILEGES FOR ROLE", ("buildDefaultACLCommands",)),
    Read(_PREAMBLE, "GRANT ", ("buildACLCommands", "dumpRoleMembership")),
    Read(_PREAMBLE, "REVOKE ", ("buildACLCommands",)),
    Read(_PREAMBLE, "SET default_tablespace", ("_selectTablespace",)),
    Read(_PREAMBLE, "\\connect ", ("appendPsqlMetaConnect",)),
    # A column's, an attribute's or a domain's type ends at one of these.
    *_clauses(
        _PREAMBLE,
        ("CREATE TABLE", "CREATE TYPE", "CREATE DOMAIN", "ALTER TABLE"),
        "COLLATE",
        "NOT",
        "DEFAULT",
        "GENERATED",
        "PRIMARY",
        "REFERENCES",
        "CHECK",
        "UNIQUE",
        "CONSTRAINT",
    ),
    # A column's or a domain's constraints.
    *_clauses(
        _PREAMBLE, ("CREATE TABLE", "CREATE DOMAIN", "ALTER TABLE"), "NULL", "NO", "KEY", "IDENTITY"
    ),
    # A table's list, its parents, and a partition's bound.
    *_clauses(_PREAMBLE, ("CREATE TABLE",), "INHERITS", "FOREIGN", "LIKE", "EXCLUDE", "USING"),
    *_clauses(_PREAMBLE, ("CREATE TABLE", "ALTER TABLE"), "FOR", "VALUES", "IN", "WITH", "FROM", "TO"),
    *_clauses(_PREAMBLE, ("ALTER TABLE",), "ONLY", "COLUMN"),
    *_clauses(_PREAMBLE, ("CREATE EXTENSION",), "IF NOT EXISTS", "SCHEMA"),
    *_clauses(_PREAMBLE, ("CREATE COLLATION",), "deterministic", "false"),
    *_clauses(_PREAMBLE, ("CREATE TYPE",), "subtype", "multirange_type_name", "canonical", "delimiter"),
    *_clauses(_PREAMBLE, ("GRANT ",), " TO "),
    *_clauses(_PREAMBLE, ("REVOKE ",), " FROM "),
)


def reads_problems(
    reads: Sequence[Read | Clause] = READS, sources: dict[str, str] | None = None
) -> list[str]:
    """The readers' keywords against [`READS`]: each recognised keyword has
    a row, each row's keyword is recognised, each `Read` names only listed
    functions, and each `Clause` lies inside a `Read` of its own reader."""
    if sources is None:
        sources = {reader: (REPO / reader).read_text() for reader in READERS}
    problems: list[str] = []
    rows: dict[tuple[str, str], Read | Clause] = {}
    for row in reads:
        key = (row.reader, row.keyword)
        if key in rows:
            problems.append(f"{row.reader}: {row.keyword!r} has two rows")
        rows[key] = row
    found = {(reader, k) for reader, text in sources.items() for k in reader_keywords(text)}
    for reader, keyword in sorted(found - rows.keys()):
        problems.append(
            f"{reader} recognises {keyword!r} and no row of READS says what writes it — "
            "a Read naming its emitters, or a Clause naming the statement it lies in"
        )
    for reader, keyword in sorted(rows.keys() - found):
        problems.append(f"{reader}: READS holds {keyword!r} and the reader no longer recognises it")
    names = listed()
    for row in reads:
        if isinstance(row, Read):
            for emitter in row.emitters:
                if emitter not in names:
                    problems.append(
                        f"{row.reader}: {row.keyword!r} is written by `{emitter}`, which "
                        "FUNCTIONS does not list"
                    )
        else:
            for keyword in row.within:
                if not isinstance(rows.get((row.reader, keyword)), Read):
                    problems.append(
                        f"{row.reader}: {row.keyword!r} lies within {keyword!r}, which is no Read "
                        "of the same reader"
                    )
    return problems


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
        if len(fields) != len(HEADER) or fields[0] not in ("literal", "option", "evidence"):
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


def extract(checkout: Path, major: str) -> tuple[list[Row], list[str]]:
    """Every row a checkout of `major` yields, or the problems that stopped
    it."""
    rows: list[Row] = []
    problems: list[str] = []
    for file, emitters in FUNCTIONS:
        path = checkout / file
        if not path.is_file():
            problems.append(f"{path}: missing")
            continue
        text = path.read_text(errors="replace")
        for emitter in emitters:
            if not emitter.at(major):
                continue
            body = function_body(text, emitter.name)
            if body is None:
                problems.append(f"{file}: no function `{emitter.name}` — the emitter has moved")
                continue
            rows.extend(literals(body, file, emitter.name))
    text_of: dict[str, str] = {}
    for file, program in OPTION_TABLES:
        path = checkout / file
        if not path.is_file():
            problems.append(f"{path}: missing")
            continue
        text_of[file] = path.read_text(errors="replace")
        found, more = options(text_of[file], file, program)
        rows.extend(found)
        problems.extend(more)
    found, more = evidence_rows(text_of, rows)
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
    rows, problems = extract(checkout, major)
    if problems:
        return problems
    (fixtures / major / REGISTER_NAME).write_text(render(rows, release))
    return []


# --------------------------------------------------------------------------
# The value-form half
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class ValueForm:
    """One spelling a backend output function writes into `COPY` text under a
    selector `pg_dump` does not pin. `spelling` is the file's bytes, `COPY`'s
    backslash doubling included; `selector` is the flag set's name in
    `generate_fixtures.SCHEMAS` that reaches it."""

    file: str  # the backend file holding the output function
    function: str  # the `*_out` function, or the one it calls that decides
    spelling: str
    selector: str  # "<schema>/<flag set>"
    what: str

    def row(self) -> Row:
        return Row("value", self.file, self.function, self.spelling, self.selector)


#: The value forms the session-setting variants and the float option reach,
#: and those a column's typmod or its type's declaration selects, hand-listed
#: (I4 names what `pg_dump` pins; everything else is a server's, or the
#: column's).
VALUE_FORMS: tuple[ValueForm, ...] = (
    ValueForm(
        "src/backend/utils/adt/varlena.c",
        "byteaout",
        "\\\\000",
        "types/bytea-output-escape",
        "`bytea_output = escape`: a byte outside printable ASCII as a three-digit octal escape",
    ),
    ValueForm(
        "src/backend/utils/adt/varlena.c",
        "byteaout",
        "\\\\\\\\backslash",
        "types/bytea-output-escape",
        "`bytea_output = escape`: a backslash byte as a doubled backslash",
    ),
    ValueForm(
        "src/backend/utils/adt/datetime.c",
        "EncodeTimezone",
        "-00:43:08",
        "types/timezone-monrovia",
        "a `TimeZone` whose offset at the instant has seconds (`Africa/Monrovia`'s LMT)",
    ),
    ValueForm(
        "src/backend/utils/adt/datetime.c",
        "EncodeDateTime",
        "23:16:52-00:43:08 BC",
        "types/timezone-monrovia",
        "an instant in the first year AD written in the year before it, the zone's offset carrying it across the era",
    ),
    ValueForm(
        "src/backend/utils/adt/float.c",
        "float4out",
        "1.17549e-38",
        "types/extra-float-digits-0",
        "`extra_float_digits = 0`: a `real` at `FLT_DIG` significant digits, not the shortest exact form",
    ),
    ValueForm(
        "src/backend/utils/adt/float.c",
        "float8out",
        "2.2250738585072e-308",
        "types/extra-float-digits-0",
        "`extra_float_digits = 0`: a `double precision` at `DBL_DIG` significant digits",
    ),
    ValueForm(
        "src/backend/utils/adt/numeric.c",
        "numeric_out",
        "\t-1.5000000000\t",
        "types/default",
        "`numeric(38,10)`: a value written with every fractional digit its scale holds (I51)",
    ),
    ValueForm(
        "src/backend/utils/adt/numeric.c",
        "numeric_out",
        "\t0.00\t",
        "types/default",
        "`numeric(10,2)`: zero written at its scale",
    ),
    ValueForm(
        "src/backend/utils/adt/varchar.c",
        "bpcharout",
        "\thi        \n",
        "types/default",
        "`char(10)`: a value padded with spaces to its length",
    ),
    ValueForm(
        "src/backend/utils/adt/timestamp.c",
        "timestamp_out",
        "2024-01-01 00:00:00.123\t",
        "types/default",
        "`timestamp(3)`: fractional seconds rounded to the precision",
    ),
    ValueForm(
        "src/backend/utils/adt/timestamp.c",
        "timestamptz_out",
        "2024-01-01 00:00:01+00",
        "types/default",
        "`timestamp(0) with time zone`: a fraction rounded up into the next second",
    ),
    ValueForm(
        "src/backend/utils/adt/date.c",
        "timetz_out",
        "12:34:56.79+02",
        "types/default",
        "`time(2) with time zone`: fractional seconds rounded to the precision",
    ),
    ValueForm(
        "src/backend/utils/adt/timestamp.c",
        "interval_out",
        "3 days 04:05:06.79",
        "types/default",
        "`interval day to second(2)`: fractional seconds rounded to the precision",
    ),
    ValueForm(
        "src/backend/utils/adt/arrayfuncs.c",
        "array_out",
        '("{""a b"";c,d;""e;f""}")',
        "emitters/default",
        "a base type's `DELIMITER = ';'`: its array's elements separated by `;`, beneath a composite (I22)",
    ),
)


def value_form_problems(forms: Sequence[ValueForm] = VALUE_FORMS) -> list[str]:
    """A value form naming a flag set the generator does not run is a problem:
    its selector is what says how the spelling is reached."""
    problems = []
    for form in forms:
        schema, _, flag_set = form.selector.partition("/")
        if flag_set not in gf.SCHEMAS.get(schema, {}):
            problems.append(f"value form {form.spelling!r}: {form.selector!r} is no flag set")
    return problems


# --------------------------------------------------------------------------
# Dispositions
# --------------------------------------------------------------------------

POSTGRES_INVARIANTS = REPO / "docs/design/postgres-invariants.md"
COMPATIBILITY = REPO / "docs/design/pg-dump-compatibility.md"


@dataclass(frozen=True)
class Invariant:
    """An `I<n>` proving that no producer the generator runs writes the row."""

    id: str


@dataclass(frozen=True)
class Known:
    """A `KD<k>` naming the row's form as a known failure."""

    id: str


@dataclass(frozen=True)
class NoOutput:
    """An option that changes no byte of the dump: `needle` is the line of the
    program's source that consumes it, which the extraction finds at every
    major holding the option and records as an `evidence` row."""

    needle: str


@dataclass(frozen=True)
class Unsupported:
    """An option whose values this build reads write only bytes some fixture
    already holds, the rest writing input it does not yet read: `row` is the
    first cell of the `docs/design/pg-dump-compatibility.md` row saying so,
    resolving only while that row's Status opens `Planned` or `Unsupported`.
    The change that makes the input read moves the row and so fails the join,
    and gives the option its flag set."""

    row: str


Reason = Invariant | Known | NoOutput | Unsupported


@dataclass(frozen=True)
class Exemption:
    """A row no fixture holds, and why. `function` is the C function for a
    literal and the program for an option, as a row's is."""

    kind: str  # "literal" or "option"
    function: str
    entry: str
    reason: Reason


def _no_output(program: str, entry: str, needle: str) -> Exemption:
    return Exemption("option", program, entry, NoOutput(needle))


#: Every row a fixture does not reach, with the reason it need not. A
#: literal may be exempt only by an `I<n>` or a `KD<k>`: every byte passes
#: through the map, so there is no "pgdt does not read this"
#: (docs/design/roadmap-P31-correctness-evidence.md, "The emitter register").
EXEMPTIONS: tuple[Exemption, ...] = (
    Exemption(
        "literal",
        "dumpTableSchema",
        "::pg_catalog.regclass AND\nconkey IN (",
        Invariant("I52"),
    ),
    Exemption("literal", "_doSetFixedOutputState", "SET ROLE ", Invariant("I87")),
    Exemption("literal", "RestoreArchive", "COMMIT;\nBEGIN;\n", Invariant("I87")),
    Exemption("literal", "_selectOutputSchema", "SET search_path = ", Invariant("I88")),
    Exemption("literal", "_selectOutputSchema", ", pg_catalog", Invariant("I88")),
    Exemption(
        "literal",
        "buildDefaultACLCommands",
        "SELECT pg_catalog.binary_upgrade_set_record_init_privs(true);\n",
        Invariant("I89"),
    ),
    Exemption(
        "literal",
        "buildDefaultACLCommands",
        "SELECT pg_catalog.binary_upgrade_set_record_init_privs(false);\n",
        Invariant("I89"),
    ),
    Exemption("option", "pg_dump", "format", Unsupported("`--format=custom`")),
    Exemption(
        "option",
        "pg_dump",
        "compress",
        Unsupported("Dump-level compression (`-Z`/`--compress`) for plain format"),
    ),
    Exemption("option", "pg_dump", "encoding", Unsupported("Non-UTF8 `client_encoding`")),
    Exemption("option", "pg_dumpall", "encoding", Unsupported("Non-UTF8 `client_encoding`")),
    _no_output("pg_dump", "dbname", "dopt.cparams.dbname = pg_strdup(optarg);"),
    _no_output("pg_dump", "host", "dopt.cparams.pghost = pg_strdup(optarg);"),
    _no_output("pg_dump", "port", "dopt.cparams.pgport = pg_strdup(optarg);"),
    _no_output("pg_dump", "password", "dopt.cparams.promptPassword = TRI_YES;"),
    _no_output("pg_dump", "no-password", "dopt.cparams.promptPassword = TRI_NO;"),
    _no_output("pg_dump", "role", "use_role = pg_strdup(optarg);"),
    _no_output("pg_dump", "file", "filename = pg_strdup(optarg);"),
    _no_output("pg_dump", "jobs", "parallel backup only supported by the directory format"),
    _no_output("pg_dump", "lock-wait-timeout", "dopt.lockWaitTimeout = pg_strdup(optarg);"),
    _no_output("pg_dump", "no-sync", "dosync = false;"),
    _no_output("pg_dump", "sync-method", "parse_sync_method(optarg, &sync_method)"),
    _no_output("pg_dump", "snapshot", "dumpsnapshot = pg_strdup(optarg);"),
    _no_output("pg_dump", "serializable-deferrable", "SERIALIZABLE, READ ONLY, DEFERRABLE"),
    _no_output("pg_dump", "no-synchronized-snapshots", "dopt.no_synchronized_snapshots"),
    _no_output("pg_dump", "no-reconnect", "no-op, still accepted for backwards compatibility"),
    _no_output("pg_dump", "help", "help(progname);"),
    _no_output("pg_dump", "version", 'puts("pg_dump (PostgreSQL) " PG_VERSION);'),
    _no_output("pg_dumpall", "dbname", "connstr = pg_strdup(optarg);"),
    _no_output("pg_dumpall", "database", "pgdb = pg_strdup(optarg);"),
    _no_output("pg_dumpall", "host", "pghost = pg_strdup(optarg);"),
    _no_output("pg_dumpall", "port", "pgport = pg_strdup(optarg);"),
    _no_output("pg_dumpall", "password", "prompt_password = TRI_YES;"),
    _no_output("pg_dumpall", "no-password", "prompt_password = TRI_NO;"),
    _no_output("pg_dumpall", "role", "use_role = pg_strdup(optarg);"),
    _no_output("pg_dumpall", "file", "filename = pg_strdup(optarg);"),
    # Forwarded to each database's `pg_dump`, whose own exemption covers it.
    _no_output("pg_dumpall", "lock-wait-timeout", '" --lock-wait-timeout "'),
    _no_output("pg_dumpall", "no-sync", "dosync = false;"),
)


def option_file(program: str) -> str | None:
    return next((file for file, name in OPTION_TABLES if name == program), None)


def evidence_rows(text_of: dict[str, str], rows: Sequence[Row]) -> tuple[list[Row], list[str]]:
    """The `evidence` rows a checkout yields: each `NoOutput` exemption whose
    option this checkout's table holds, with its needle found in the file
    holding the program's `main`. A needle not found is a problem."""
    out: list[Row] = []
    problems: list[str] = []
    present = {(r.function, r.entry) for r in rows if r.kind == "option"}
    for ex in EXEMPTIONS:
        if not isinstance(ex.reason, NoOutput) or (ex.function, ex.entry) not in present:
            continue
        file = option_file(ex.function)
        if file is None or ex.reason.needle not in text_of.get(file, ""):
            problems.append(
                f"{file}: no {ex.reason.needle!r} — `{ex.function} --{ex.entry}`'s evidence has moved"
            )
            continue
        out.append(Row("evidence", file, ex.function, ex.entry, ex.reason.needle))
    return out, problems


def reason_problems(
    exemptions: Sequence[Exemption] = EXEMPTIONS,
    invariants: str | None = None,
    compatibility: str | None = None,
    open_kds: set[str] | None = None,
) -> list[str]:
    """Each exemption's reason resolved against the committed record: an
    `I<n>` to a heading, a `KD<k>` to an open entry, a compatibility row to a
    row of that table still saying its input is not read. A literal exempt by anything but an `I<n>` or a `KD<k>`
    is a problem."""
    if invariants is None:
        invariants = POSTGRES_INVARIANTS.read_text()
    if compatibility is None:
        compatibility = COMPATIBILITY.read_text()
    if open_kds is None:
        import deficiencies

        entries, _ = deficiencies.parse_index(deficiencies.REGISTER.read_text())
        open_kds = {e.id for e in entries}
    problems = []
    for ex in exemptions:
        name = f"{ex.kind} {ex.function} {ex.entry!r}"
        reason = ex.reason
        if ex.kind == "literal" and not isinstance(reason, (Invariant, Known)):
            problems.append(f"{name}: a literal is exempt only by an I<n> or a KD<k>")
        if isinstance(reason, Invariant):
            if not re.search(rf"^## {re.escape(reason.id)} —", invariants, re.MULTILINE):
                problems.append(f"{name}: {reason.id} is no entry of postgres-invariants.md")
        elif isinstance(reason, Known):
            if reason.id not in open_kds:
                problems.append(f"{name}: {reason.id} is no open entry of deficiencies.md")
        elif isinstance(reason, Unsupported):
            row = re.search(
                rf"^\| {re.escape(reason.row)} \| ([^|]*)\|", compatibility, re.MULTILINE
            )
            if row is None:
                problems.append(f"{name}: no pg-dump-compatibility.md row {reason.row!r}")
            elif not row.group(1).strip().startswith(("Planned", "Unsupported")):
                problems.append(
                    f"{name}: pg-dump-compatibility.md's {reason.row!r} row reads "
                    f"{row.group(1).strip()!r}, so the input is read now; give the option a flag set"
                )
    return problems


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
            if isinstance(flags, list):
                out["pg_dump"].extend(flags)
            elif isinstance(flags, gf.Dumpall):
                out["pg_dumpall"].extend(flags.flags)
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
    values: int = 0
    #: Rows no fixture reaches and no exemption covers: the gate fails on one.
    uncovered: list[Row] = field(default_factory=list)
    #: Rows no fixture reaches, each with the exemption that covers it.
    exempt: list[tuple[Row, Exemption]] = field(default_factory=list)
    #: Exemptions whose row a fixture of this major reaches.
    stale: list[Exemption] = field(default_factory=list)


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
    evidence = {(r.function, r.entry): r.detail for r in rows if r.kind == "evidence"}
    exemptions = {(ex.kind, ex.function, ex.entry): ex for ex in EXEMPTIONS}
    for row in rows:
        if row.kind == "literal":
            result.literals += 1
            needle = row.entry.encode("latin-1")
            reached = any(needle in dump for dump in dumps)
        elif row.kind == "option":
            result.options += 1
            reached = row.entry in covered_options.get(row.function, set())
        else:
            continue
        ex = exemptions.get((row.kind, row.function, row.entry))
        if reached:
            if ex is not None:
                result.stale.append(ex)
        elif ex is None:
            result.uncovered.append(row)
        else:
            result.exempt.append((row, ex))
            if isinstance(ex.reason, NoOutput) and evidence.get((row.function, row.entry)) != ex.reason.needle:
                problems.append(
                    f"{path}: no evidence row for `{row.function} --{row.entry}` holding its "
                    "exemption's needle — re-run `emitter_register.py --extract`"
                )
    for form in VALUE_FORMS:
        result.values += 1
        if not any(form.spelling.encode() in dump for dump in dumps):
            result.uncovered.append(form.row())
    return result, problems


def exemption_problems(
    results: Sequence[MajorResult], exemptions: Sequence[Exemption] = EXEMPTIONS
) -> list[str]:
    """An exemption is wrong where a fixture reaches its row, and dead where
    no major's register holds the row: either way it is struck."""
    problems = []
    used = {id(ex) for r in results for _, ex in r.exempt}
    for r in results:
        for ex in r.stale:
            problems.append(
                f"{r.major}: {ex.kind} {ex.function} {ex.entry!r} is reached by a fixture "
                "and still exempt — strike the exemption"
            )
    for ex in exemptions:
        if id(ex) not in used and not any(ex in r.stale for r in results):
            problems.append(f"{ex.kind} {ex.function} {ex.entry!r}: exempt, and no register holds it")
    return problems


def majors() -> list[str]:
    return sorted(gf.ROUTINE_VERSIONS, key=int)


def report(results: Sequence[MajorResult]) -> str:
    lines = []
    for r in results:
        n_lit = sum(1 for u in r.uncovered if u.kind == "literal")
        n_opt = sum(1 for u in r.uncovered if u.kind == "option")
        n_val = sum(1 for u in r.uncovered if u.kind == "value")
        x_lit = sum(1 for u, _ in r.exempt if u.kind == "literal")
        x_opt = sum(1 for u, _ in r.exempt if u.kind == "option")
        lines.append(
            f"{r.major}: {r.literals} literals, {n_lit} uncovered, {x_lit} exempt; "
            f"{r.options} options, {n_opt} uncovered, {x_opt} exempt; "
            f"{r.values} value forms, {n_val} uncovered"
        )
    where: dict[tuple[str, str, str, str], list[str]] = defaultdict(list)
    for r in results:
        for u in r.uncovered:
            where[(u.kind, u.file.rsplit("/", 1)[-1], u.function, u.entry)].append(r.major)
    lines.append("")
    if not where:
        lines.append("every row is held by a fixture or exempt.")
        return "\n".join(lines)
    lines.append("uncovered, which fails the join (majors):")
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
    problems = value_form_problems() + reason_problems() + reads_problems()
    for major in chosen:
        result, more = join_major(major)
        results.append(result)
        problems.extend(more)
    if args.major is None:
        problems.extend(exemption_problems(results))
    print(report(results), file=out)
    for p in problems:
        print(f"problem: {p}", file=sys.stderr)
    return 1 if problems or any(r.uncovered for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
