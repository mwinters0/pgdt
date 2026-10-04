# P31.29 — A `COPY` row held to its block's line ending: notes

What the slices after this one inherit. The external fact is I91; why line
endings follow PostgreSQL's two readers is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "Line
endings follow PostgreSQL's two readers", and why the bare-CR half is
unowned is the same day's "31.29 lands without bare-CR blocks; `KD101`
unowned".

## What exists

- **`RowEnding`, in `scan.rs`**, is how a row's line ends as
  `CopyReadLineText` reads it: by its first CR no backslash escapes, an odd
  run of backslashes before a CR making it data. A row with no CR is found by
  one `memchr2` pass and classified for free; only a line holding a CR is
  walked. That search replaces a `memchr` on every row, and no scan figure
  was re-taken over it (`measure.py --stale` says which are red).
- **The scanner holds a block's rows to its first row's ending only where it
  entered the block at its header**, and raises `Error::RowEndingRefused`,
  naming the table and `COPY`'s line, as `literal carriage return found in
  data` or `literal newline found in data`. That is every mapping pass's serial
  path. A scanner resumed inside a block checks nothing: every replay reads a
  block a mapping pass checked. The refusal ignores
  `--postgres-invalid-values`, being framing rather than a value.
- **A split block is held by the leader** (`leader.rs`, `BlockEndings`): each
  piece records its first row's ending and its first row ending otherwise,
  and `run_region` holds the pieces to the block's first row as each
  partition returns, in file order, so the refusal raised is the one a serial
  scan raises and is raised ahead of any later partition's error. Pieces past
  the terminator, which scan DDL as rows, are held to nothing.
- **The `\.` line ends a block in either ending, whatever its rows end in**:
  psql finds it, and only 15–17 send it to a server refusing a mismatch, so
  pgdt reads it as the most permissive restore does (`roadmap.md`, "A literal
  is guaranteed in `*_out`'s form and never read past `*_in`'s").
- **A row's raw bytes keep a CR a backslash escapes** where they lost it
  before (`2\<CR><LF>` was read as `2\`), so `CACHE_FORMAT_VERSION` is 68. No
  fixture holds a CR in a row, so both pinned digests stayed.
- **Evidence**: `scan.rs`'s `a_row_ending_otherwise_than_its_block_s_first_is_refused`
  and `the_rows_a_restore_reads_are_read_without_their_endings`, at chunk sizes
  down to one byte; `leader.rs`'s
  `a_split_block_refuses_the_row_a_serial_scan_refuses` at 2, 3 and 8 jobs,
  with a raw CR in a literal after the block; `tests/scan.rs`'s
  `a_crlf_conversion_reads_alike_and_a_stray_ending_is_refused`, the 16 `types`
  fixture converted to CR LF, read and refused serially and split. The koji
  replica answered every form I91 names under psql 16.

## Findings

- **`KD104`, `(c)`**: a backslash before a raw LF or tab makes it data, as
  `CopyReadLineText` and `CopyReadAttributesText` read it; the scanner ends a
  row at every LF and `split_fields` splits at every tab. `pg_dump` escapes
  both (I15).
- **`KD105`, `(c)`**: a `\.` at the file's end with no LF after it ends its
  block, `missing_trailing_newline_is_tolerated` asserting it, where a restore
  refuses it, rows or none.

## What a reader of bare-CR blocks inherits

- `RowEnding::Cr` is the block's ending where its first row's first
  unescaped CR is not its line's last byte; `refused_in` returns `None` for
  every row of such a block, and the `KD101` marker sits on that arm.
- The serial scanner frames rows by LF; the leader's resync
  (`scan_piece`) and a query segment's (`stream.rs`, `first_row_start`)
  search for an LF, which a bare-CR block's interior does not hold.
