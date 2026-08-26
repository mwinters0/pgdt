# 9.3 — The coverage line

The phase's one user-facing formatting decision, kept as its own slice so it
can come back after a look without re-reviewing the `info` rework.

## What prints

```
Scan completion: 76% (12345 bytes)
```

First line of every `info` listing, partial or complete, followed by a blank
line. `completion_line(scanned_through, total_size)` builds it and is the only
place the percentage exists.

The percentage **floors** (integer division), so it reads `100%` only for a
genuinely finished scan — a 99.6% scan reads 99. That is the property worth
preserving if the format is ever revisited: a user checking whether an
interrupted koji parse finished must not be told 100% by rounding.

`total_size == 0` yields 100%: a zero-byte file is trivially covered in full
and has no ratio. `checked_div` rather than a branch, so clippy stays quiet.

## Three decisions inside it

**Unconditional, not only when partial.** "How much of this file do we know"
is not a question only a partial answer raises, and a line that appears only
sometimes is one a script (or a reader) has to test for.

**Byte counts left the summaries below it.** `print_index`'s trailing line is
now `N COPY block(s), M row(s)`, `--map`'s is `N span(s)`, and the empty case
is `no COPY blocks found`. Stating the same number twice in one listing invites
the two to disagree; the coverage line owns it.

**JSON carries components, not the rendered string.** `IndexJson` gained
`total_size`; `scanned_through` was already flattened out of `DumpIndex`. A
script computes whatever ratio it wants and never parses the text line back
apart. This is also why `total_size` went onto `CacheStatus::Valid` in 9.2 —
the export needs it for a complete cache as much as for a partial one.

## Where it is rendered from

`report(index, total_size, verbose, map, json)` is the single entry point every
`pgdq info` rendering goes through — both invocation forms, all four flag
combinations. It prints the coverage line, then delegates to `print_index` or
`print_index_json`. `print_index` no longer states coverage at all, and its
doc-comment says why: a partial index lacks records, not confidence, so
repeating the caveat per block would suggest a variation that does not exist.
