#!/usr/bin/env python3
"""Generate a pg_dump-shaped dump whose one table holds a cluster of long lines.

Backs the decode factors (`docs/design/decisions.md`, "D154"): the
`long-line-resident` figure (`measure.py`) reads what a pass holds resident
while it holds a line, by pass kind and line length, so its input is a block
of ordinary short rows around a **cluster of consecutive rows each exactly
`--line-bytes` long, its terminator included** -- I71's measure
(`docs/design/postgres-invariants.md`). Consecutive, because the reference
dump's long lines arrive that way (three to nine over 64 MiB in one chunk), and
a cluster at least as long as a figure's worker count is what lets each worker
of a parallel pass hold one at once.

One value kind a file, `--kind`, since what a decoding pass holds beside the
line depends on how far decoding shortens the value:

- `text`: unescaped letters, which decode to themselves;
- `escaped`: text with a `\\n` escape every eight bytes, decoding to seven;
- `bytea`: hex output (`\\\\x...` in a `COPY` line), decoding to half;
- `int8array`: a `bigint[]` of single digits, `{1,2,...}`, the densest text a
  fixed-width element's form is decoded from;
- `textarray`: a `text[]` of one-letter elements, `{a,b,...}`, each part's
  token and unquoted copy held beside its view;
- `jsonb`: an object of empty keys, `{"":0,"":0,...}`, the densest text a
  `jsonb` ordering key holds a pair's position for -- valid input, though
  `jsonb_out` writes no duplicate key.

`row_count` is what `SELECT count(v)` answers over the file, which the figure
holds each query leg to, so a leg that read no row, or lost one, is refused
rather than published.

Not a correctness fixture: shaped closely enough to satisfy this codebase's
grammar, as `generate_block_count_bench.py` is. Deterministic: nothing is
random, so no seed is taken. Generated, never committed.
"""

from __future__ import annotations

import argparse
from pathlib import Path

MIB = 1 << 20

#: The value kinds, in the figure's row order.
KINDS: tuple[str, ...] = ("text", "escaped", "bytea", "int8array", "textarray", "jsonb")

#: The type each kind's `v` column is declared.
COLUMN_TYPE: dict[str, str] = {
    "text": "text",
    "escaped": "text",
    "bytea": "bytea",
    "int8array": "bigint[]",
    "textarray": "text[]",
    "jsonb": "jsonb",
}

TABLE = "public.long_lines"

#: Short rows before the cluster and after it, so the block opens and closes on
#: ordinary lines.
SHORT_ROWS = 8

#: The cluster's ids: three digits each, so every long line's field is the same
#: length, and a bytea field's hex digits even (`field_bytes`).
FIRST_LONG_ID = 101
MAX_LINES = 99

#: The ids of the short rows after the cluster.
TAIL_FIRST_ID = 901

#: Each kind's short value as `COPY` writes it, and its decoded length.
SHORT_VALUE: dict[str, tuple[bytes, int]] = {
    "text": (b"short", 5),
    "escaped": (b"a\\tb", 3),
    "bytea": (b"\\\\x00ff", 2),
    "int8array": (b"{1,2}", 5),
    "textarray": (b"{a,b}", 5),
    "jsonb": (b'{"a": 1}', 8),
}

#: Each kind's repeating unit of a long field as `COPY` writes it, and what one
#: unit decodes to.
_TEXT_UNIT = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
_ESCAPED_UNIT = b"abcdef\\n"
_HEX_UNIT = b"0123456789abcdef"
#: What the `bytea` field opens with: `\x`, its backslash escaped.
_HEX_PREFIX = b"\\\\x"
#: Each array kind's repeating unit inside its braces, an element and its
#: comma, and the element it closes on.
_ARRAY_UNIT: dict[str, tuple[bytes, bytes]] = {
    "int8array": (b"1,2,3,4,5,6,7,8,", b"9"),
    "textarray": (b"a,b,c,d,e,f,g,h,", b"z"),
}
#: The `jsonb` field's repeating pair and its comma, and the head of the pair
#: it closes on, whose value's digits make up the field's exact length.
_JSONB_PAIR = b'"":0,'
_JSONB_LAST = b'"":'
#: The most digits the closing pair's value takes: one pair's length.
_JSONB_DIGITS = len(_JSONB_PAIR)

HEADER = """\
--
-- PostgreSQL database dump
--

-- Dumped from database version 16.15
-- Dumped by pg_dump version 16.15

SET statement_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET row_security = off;

--
-- Name: long_lines; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE {table} (
    id integer,
    v {column_type}
);


ALTER TABLE {table} OWNER TO postgres;

--
-- Data for Name: long_lines; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY {table} (id, v) FROM stdin;
"""

FOOTER = """\
\\.


--
-- PostgreSQL database dump complete
--

"""


def field_bytes(kind: str, line_bytes: int) -> int:
    """The bytes of a long row's `v` field: the line less its three-digit id,
    the tab and the terminator. Refused where the kind cannot fill it exactly:
    a `bytea` field is its prefix and an even count of hex digits, an array's
    an odd count, its elements and commas alternating inside its braces."""
    if kind not in KINDS:
        raise ValueError(f"no value kind {kind!r}; one of {', '.join(KINDS)}")
    n = line_bytes - len(str(FIRST_LONG_ID)) - 2
    floor = {
        "bytea": len(_HEX_PREFIX),
        "int8array": 3,
        "textarray": 3,
        "jsonb": 2 + len(_JSONB_LAST) + 1,
    }.get(kind, 1)
    if n < floor:
        raise ValueError(f"a {line_bytes}-byte line leaves no room for a {kind} field")
    if kind == "bytea" and (n - len(_HEX_PREFIX)) % 2:
        raise ValueError(f"a {line_bytes}-byte line leaves an odd count of hex digits")
    if kind in _ARRAY_UNIT and n % 2 == 0:
        # Braces around elements and commas alternating, an element at each end.
        raise ValueError(f"a {line_bytes}-byte line leaves an even {kind} field")
    return n


def decoded_bytes(kind: str, n: int) -> int:
    """What a long field of `n` bytes as `COPY` writes it decodes to."""
    if kind == "text":
        return n
    if kind == "escaped":
        units, rest = divmod(n, len(_ESCAPED_UNIT))
        return units * (len(_ESCAPED_UNIT) - 1) + rest
    if kind == "bytea":
        return (n - len(_HEX_PREFIX)) // 2
    if kind in _ARRAY_UNIT or kind == "jsonb":
        # No escape: its text is its `COPY` field.
        return n
    raise ValueError(f"no value kind {kind!r}")


def row_count(lines: int) -> int:
    """The rows of a file whose cluster is `lines` long, each holding a value."""
    return 2 * SHORT_ROWS + lines


def _repeat(f, unit: bytes, n: int) -> None:
    """Write `n` bytes of `unit` repeated, a mebibyte at a time; `n` a whole
    count of units."""
    assert n % len(unit) == 0, (n, unit)
    chunk = unit * (MIB // len(unit))
    whole, rest = divmod(n, len(chunk))
    for _ in range(whole):
        f.write(chunk)
    f.write(unit * (rest // len(unit)))


def _write_array(f, kind: str, n: int) -> None:
    """A `{e,e,...,e}` field of exactly `n` bytes, `n` odd: whole units, then
    the unit's leading elements and their commas, then its closing element."""
    unit, last = _ARRAY_UNIT[kind]
    body = n - 2 - len(last)
    units, tail = divmod(body, len(unit))
    f.write(b"{")
    _repeat(f, unit, units * len(unit))
    f.write(unit[:tail])
    f.write(last + b"}")


def _write_jsonb(f, n: int) -> None:
    """A `{"":0,...,"":<digits>}` field of exactly `n` bytes: whole pairs,
    then a closing pair whose value has the one to `_JSONB_DIGITS` digits
    left over."""
    pairs, digits = divmod(n - 2 - len(_JSONB_LAST) - 1, len(_JSONB_PAIR))
    f.write(b"{")
    _repeat(f, _JSONB_PAIR, pairs * len(_JSONB_PAIR))
    f.write(_JSONB_LAST + b"1" + b"0" * digits + b"}")


def _write_field(f, kind: str, n: int) -> None:
    """Write a long field of exactly `n` bytes, a mebibyte at a time."""
    if kind in _ARRAY_UNIT:
        _write_array(f, kind, n)
        return
    if kind == "jsonb":
        _write_jsonb(f, n)
        return
    if kind == "bytea":
        f.write(_HEX_PREFIX)
        n -= len(_HEX_PREFIX)
        unit = _HEX_UNIT
    else:
        unit = _TEXT_UNIT if kind == "text" else _ESCAPED_UNIT
    chunk = unit * (MIB // len(unit))
    whole, rest = divmod(n, len(chunk))
    for _ in range(whole):
        f.write(chunk)
    units, tail = divmod(rest, len(unit))
    f.write(unit * units)
    # The tail is a prefix of no escape: plain letters, or hex digits, whose
    # count is even (`field_bytes`).
    f.write(_HEX_UNIT[:tail] if kind == "bytea" else b"a" * tail)


def generate(out: Path, kind: str, line_bytes: int, lines: int) -> None:
    if not 1 <= lines <= MAX_LINES:
        raise ValueError(f"a cluster is 1 to {MAX_LINES} lines, not {lines}")
    n = field_bytes(kind, line_bytes)
    column_type = COLUMN_TYPE[kind]
    short = SHORT_VALUE[kind][0]
    with out.open("wb") as f:
        f.write(HEADER.format(table=TABLE, column_type=column_type).encode())
        for i in range(1, SHORT_ROWS + 1):
            f.write(b"%d\t%s\n" % (i, short))
        for i in range(FIRST_LONG_ID, FIRST_LONG_ID + lines):
            f.write(b"%d\t" % i)
            _write_field(f, kind, n)
            f.write(b"\n")
        for i in range(TAIL_FIRST_ID, TAIL_FIRST_ID + SHORT_ROWS):
            f.write(b"%d\t%s\n" % (i, short))
        f.write(FOOTER.encode())


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--kind", choices=KINDS, required=True, help="The long values' kind")
    parser.add_argument(
        "--line-bytes",
        type=int,
        required=True,
        help="Each long line's length in bytes, its terminator included",
    )
    parser.add_argument(
        "--lines", type=int, required=True, help=f"Long lines in the cluster, 1 to {MAX_LINES}"
    )
    parser.add_argument("out", type=Path, help="Output path for the generated dump")
    args = parser.parse_args()

    args.out.parent.mkdir(parents=True, exist_ok=True)
    generate(args.out, args.kind, args.line_bytes, args.lines)
    print(f"wrote {args.out} ({args.out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
