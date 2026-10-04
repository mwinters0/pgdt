# P31.28 — A raw CR inside a quoted literal kept: notes

What the slices after this one inherit. Why line endings follow PostgreSQL's
two readers is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "Line
endings follow PostgreSQL's two readers"; the external fact is I90. The entry
it closes was `KD100`.

## What exists

- **`CopyScanner::next_event` lexes the line outside a block with its CR**,
  and surfaces it with the CR where the line ends inside a quoted literal or
  `"…"` identifier, without it everywhere else. Structure (a `COPY` header,
  `BEGIN;`, `SET standard_conforming_strings`) is still matched on the
  stripped line, and a `COPY` row or large-object line is stripped as before.
  So a statement's text holds the CR LF the server holds, in an LF file
  written by `appendStringLiteral` and in the same file converted to CRLF.
- **`CACHE_FORMAT_VERSION` is 67**: an enum's labels, a `CHECK`'s name and a
  partition's bound are persisted and may now hold the CR. No fixture holds
  one, so both pinned digests stayed and only their version moved.
- **Evidence**: `tests/preamble.rs`'s
  `a_raw_cr_inside_a_literal_is_the_value_s_own_byte` — the label, the
  `CHECK` name and the bound in both files, a strict parse reading the
  label's field and refusing it without its CR; it fails with the CR
  stripped. `scan.rs`'s `a_raw_cr_ending_a_line_is_kept_only_inside_a_quoted_region`
  holds the surfaced lines at chunk sizes down to one byte. The koji replica
  answered I90's probes under `psql -f`.

## Findings

- **`KD103`, `(c)`**: psql's lexer ends a `--` comment at a CR inside a line,
  and takes one as the newline a literal's continuation needs; `lex.rs`
  takes only the line's end. `pg_dump` writes no CR outside a literal.

## What the slices after this inherit

- **31.29** checks a `COPY` row's terminator: the `InCopy` arm reads the
  stripped `line`, and `unstripped`, the line with its CR, is there beside it,
  read today by the `Outside` arm alone.
