# P7.5 — the `INSERT` run's statement scan

What the next slice inherits from `KD9`'s partial discharge — the
accumulation is gone, the entry stays live at its 4.3× residual. The mechanism itself is
filed by subject: [`architecture.md`](architecture.md), "Bulk regions: one span
kind, three payloads" for the fast path and what it declines,
"The `INSERT` path is one `memchr`-bound scan in L1" for the before-and-after
profile.

## The primitive, and why it is shaped for a caller that does not exist yet

`preamble::StatementScan` replaces `scan_buf`. It is an incremental,
byte-level, quote-aware scan: paren depth, in-string, in-quoted-identifier,
in-comment, and the last non-whitespace byte, updated by `feed(&[u8])` and
queried by `complete()`/`in_quote()`. `statement_complete(&str)` and
`in_open_quote(&str)` are now thin wrappers over it, so the rule has one
implementation and the existing `Mode::Statement` path answers exactly what it
answered before.

Three decisions in it are worth knowing:

**It carries a `Pending` across `feed` calls.** `''`, `""` and `--` are
two-byte tokens, so a caller that splits a statement between the halves of one
gets the wrong answer from a scan that cannot look back. The spec's obligation
was that the primitive be reusable by P8 Track A's row reader, and a row reader
walks *chunks*, not lines — so the straddle is handled rather than documented
away. `a_statement_scan_gives_the_same_answer_at_every_chunk_boundary` asserts
it at every split point of eleven statements. A pending quote resolves as
*closing* when queried, which is what the end of the buffer meant to the
`chars().peekable()` scan it replaces.

**`feed_line` joins with `\n` between lines and never before the first**, which
is `push_stmt_line`'s rule exactly. Appending the newline *after* each line
instead would look equivalent and is not: it closes a trailing `-- comment`,
and a buffer whose last line is a bare comment must stay open — that is what
`Mode::Statement`'s and `Mode::InsertRun`'s dangling-close arms read.

**The trailing-`;` test is ASCII whitespace, where `str::trim_end` was
Unicode.** A non-ASCII Unicode space after a statement's `;` therefore reads as
incomplete where it used to read as complete. `pg_dump` writes none, and the
consequence of being wrong is the graceful-degradation path the map already
takes for an unterminated statement, so the divergence is documented on
`is_sql_space` rather than worked around.

## What the fast path declines, and why that list is the safety argument

`Builder::insert_run_line` runs *before* `feed_line`'s UTF-8 conversion and
before its `looks_like_toc_name_line`/`partition_root_marker` prologue. That is
only sound because every line either of those could act on is declined:

- first non-ASCII-whitespace byte not ASCII — so a line with leading Unicode
  whitespace, which `str::trim` would strip and `trim_ascii_start` would not,
  never takes the fast path;
- first such byte is `-` — every boundary signal begins `--`;
- no such byte at all (a blank line);
- at a statement boundary, a line that does not restate the run's own
  `INSERT INTO <table>` byte prefix followed by whitespace or `(`.

The prefix test is a **conservative stand-in** for `parse_insert_target`, not
an equivalent: matching proves the line parses to the same qualified name
(same bytes, same parser), and not matching costs only the ordinary path, which
parses it properly. That asymmetry is what lets the check be a `starts_with`
instead of a parse. `a_table_whose_name_extends_the_runs_own_starts_a_new_run`
pins the delimiter half — `public.widgets` must not swallow `public.widgets2`.

`Mode::InsertRun` now holds a `StatementScan` and that byte prefix in place of
its `String` buffer. It can, because a run's span carries a table name and a
row count and nothing reads its text: `push_insert_run` goes straight to
`push_span`, and `extract_statement_cross_refs` is deliberately not run over an
`INSERT` body.

**Both paths feed the same scan and every line reaches exactly one of them**,
so which path saw a line never changes how the run folds.
`an_insert_run_folds_the_same_way_when_its_lines_take_the_slow_path` is the
test that says so, over a run whose lines alternate.

## `step` returns `bool`

It returned `Option<(u64, String)>` for "reprocess this line", always with the
same offset and the same line it was given — so the caller re-supplies both and
the return is a flag. That removes a `String` allocation at every mode
transition and, with `feed_line` holding a `Cow` instead of calling
`into_owned()`, the per-line allocation on the ordinary path too. Mechanical
and type-checked; it is in this slice because it is the other half of "no
`String` per line".

## What it bought, and what is left

Warm, 3.00 GiB, in the 512 MB container: **9.19 s → 2.27 s**, against a `COPY`
scan's 0.532 s — 16.5× the per-byte CPU down to **4.3×**, and 27.7× the `dd`
floor down to 7.5×. Cold on the SSD it is **gone**: 5.87 s against the floor's
5.75, 1.02× where a `COPY` scan is 1.01×.

On the host, outside the container, the same before/after pair reads 7.5 s →
2.26 s wall and 7.2 s → 1.94 s user, with the control `COPY` `parse` unmoved at
0.55 s — which is the check that the win is the `INSERT` path and not the
apparatus.

**The remainder is `memchr`, and the two cuts below are what `KD9` now names.**
In the
after-profile, `Builder::feed_line` → `insert_run_line` →
`StatementScan::feed_line` is 74.3% of user time, `CopyScanner::next_event`
14.1%, and nearly 80% of the flat profile is `memchr` across the four needle
widths `feed` uses. `feed`'s own non-SIMD code is 6.9%. Two cuts were
considered and not taken, and a later slice that wants more should start from
them: feeding only the bytes *past* the already-matched `INSERT INTO <table>`
prefix (whose scan-state delta is provably nothing, since `parse_ident`
guarantees a balanced quoted identifier), and dropping paren-depth tracking,
which no `INSERT` statement needs but the shared rule does.

## The figure was re-taken in one sitting, and that pulled a third table in

`scan-throughput-cold` and `scan-throughput-warm` were taken together —
`measurements.md` already said they come from one command — and
`census-brace-free` came with them, because each throughput table's `COPY` row
*is* the census table's census-on column, not a second measurement of it. Three
tables now stand outside the `ba2fc12` stamp for that reason and each says so.

One subtraction is now **cross-sitting** as a result: `census-arrays`' prose
takes the pre-filter's per-row cost out of its 1.49 µs, and that number moved
36 ns → 60 ns between the sweep and this sitting. It survives being
cross-sitting because the conclusion does not turn on which value is used —
96% against 98% — and the section says so rather than quietly differencing two
apparatuses. `census-arrays` was not re-taken: this slice does not touch the
census, and re-taking a table because its neighbour moved is how a per-figure
re-take becomes a sweep.

## Deliberately not done

- **`Mode::Statement` still accumulates a `String` and still re-walks it per
  line.** Its span carries the statement's text, which `classify_statement` and
  `extract_statement_cross_refs` read, so the buffer cannot go; the per-line
  re-walk could be made incremental with a second `StatementScan` beside the
  buffer. It is a few megabytes of DDL in a file of gigabytes, and this slice
  is `KD9`.
- **The `--inserts` *row reader* is still P8 Track A.** What landed is where a
  statement ends, cheaply and reusably; nothing here parses a value.
- **No sweep.** Three figures were re-taken with `--figure`; the other eleven
  keep the `ba2fc12` stamp, and `STATUS.md` says which of them read stale and
  why.

## Apparatus

| Path | What it is |
|---|---|
| `runs/measure-20260903T190630/` | the sitting the three re-taken tables come from — `tables.md` is what was folded into `measurements.md` |
| `runs/profile-parse-insert_run-p75.{data,txt}` | the after-profile, frame-pointer build, taken with the sequence `measure.py --profile-recipe` prints |
| `runs/pgdq-7.5-{before,after}` | `release` builds either side, for the host-side sanity pair above; "before" was built in a throwaway worktree at `843f53e` |
| `runs/p75-{warm,cold,both}.log` | the harness runs; only the third produced the published tables |

The strongest regression check this slice ran is not in `runs/`: both binaries
were run over **all 109 fixture `.sql` files** — `parse` then `info --map
--verbose` — and every pair is byte-identical, `edge_cases/inserts.sql` and
`edge_cases/column-inserts.sql` included. Repeat it with any two builds; it
costs seconds and it is what says the map did not move.
