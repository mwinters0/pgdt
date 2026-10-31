#!/usr/bin/env python3
"""Generate a pg_dump-shaped dump whose one table holds a cluster of long lines.

Backs `docs/design/roadmap-P33-line-bound.md`, "How it is shown": the
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
- `bytea`: hex output (`\\\\x...` in a `COPY` line), decoding to half.

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
KINDS: tuple[str, ...] = ("text", "escaped", "bytea")

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
}

#: Each kind's repeating unit of a long field as `COPY` writes it, and what one
#: unit decodes to.
_TEXT_UNIT = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
_ESCAPED_UNIT = b"abcdef\\n"
_HEX_UNIT = b"0123456789abcdef"
#: What the `bytea` field opens with: `\x`, its backslash escaped.
_HEX_PREFIX = b"\\\\x"

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
    a `bytea` field is its prefix and an even count of hex digits."""
    if kind not in KINDS:
        raise ValueError(f"no value kind {kind!r}; one of {', '.join(KINDS)}")
    n = line_bytes - len(str(FIRST_LONG_ID)) - 2
    floor = len(_HEX_PREFIX) if kind == "bytea" else 1
    if n < floor:
        raise ValueError(f"a {line_bytes}-byte line leaves no room for a {kind} field")
    if kind == "bytea" and (n - len(_HEX_PREFIX)) % 2:
        raise ValueError(f"a {line_bytes}-byte line leaves an odd count of hex digits")
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
    raise ValueError(f"no value kind {kind!r}")


def row_count(lines: int) -> int:
    """The rows of a file whose cluster is `lines` long, each holding a value."""
    return 2 * SHORT_ROWS + lines


def _write_field(f, kind: str, n: int) -> None:
    """Write a long field of exactly `n` bytes, a mebibyte at a time."""
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
    column_type = "bytea" if kind == "bytea" else "text"
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
