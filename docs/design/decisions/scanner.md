# The scanner

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. The `COPY` scanner: what it treats as structure, why it
owns no bytes and validates UTF-8 once per chunk, how a scan is cancelled, and the row-path optimizations measured and
refused. `pgdt-dump`'s `scan` and `pgdt-query`'s batch path.

### D23 The scanner never owns the bytes it scans, and only the whole `COPY` grammar is structural
`CopyScanner` is a synchronous state machine over a caller-owned buffer, so a chunk is scanned where it lies, only the
line straddling its edge copied (rejected: a growing buffer, copying every byte twice), and that line frozen for its
row, a replay's block, then dropped (rejected: reusing its buffer, holding the longest line's bytes all pass). Past
`max_line_bytes` a line errors, never truncates (`KD93`). Only a line matching the whole `COPY … FROM stdin;` grammar,
or a bare `BEGIN;` (D33), is structural; inside a block only an exact `\.` line and a row's ending are read (I7, I91).
Outside one every line is lexed as psql lexes it (I50), one beginning inside a string, identifier, comment or
dollar-quoted body never structure (I1). Rejected: tracking `$` alone, or quotes too (a `$$` in a name or comment hid
every block).

### D138 A row its reader only counts is streamed once it outgrows its chunk
A block whose rows nothing reads past their count — no census, observer or record: a metadata-level parse and its
interior workers — streams them: a row no chunk ends is folded into a `StreamedLine`, its length and ending (I91), not
carried, and handed out as `Event::StreamedRow`, so the pass holds no line at any length. One a chunk ends, or no
longer than `\.` and a CR, stays a `Row`. The state is `ChunkCarry`'s, shared by the serial loop and a worker, whose
scanner is new each span; a block holds unless its reader says otherwise. Rejected: a `Row` with an empty `raw`, which
a reader decoding it cannot tell from an empty row; streaming in the scanner, which a worker's spans would lose.
Reopens: a census reading a bounded head of each field. Evidence: `crates/pgdt-mapping/tests/streamed_line.rs`.

### D139 A line past its chunk grows in a mapping of its own, by `mremap`, in huge-page steps
A carried line longer than its chunk leaves the carry's `Vec` for an anonymous mapping (`LineMapping`), grown by
`mremap` in steps of the PMD huge page where at most `STEP_CEILING` (else the base page), its start aligned to one,
moved only to an aligned range it reserved, and unmapped with its last `Bytes`: commit and resident are its bytes and
under a step, on any allocator, and THP can back a step across a move. Rejected: growing through the allocator (a
doubling's capacity; mimalloc's unpurged buffers; D13; `KD148`); exact growth, faulted in base pages; mapping a line at
its block's longest; I71's bound reserved per line, counted whole by an address-space limit; a larger PMD page;
`MADV_HUGEPAGE`. Linux's; else a `Vec` (P34). Reopens: a material gap to a sized mapping. Evidence: RT29, RT30;
`the_carry_keeps_no_line_past_its_row`.

### D25 Parallelize what is CPU-bound
One core's rate against the device's offer decides each kind of work (decode, extraction, discovery) per source; D2
holds the answers, D7 who asks, D52 the worker. Readings that disagree reopen it. Evidence: `xz-decode-scaling`,
`parallel-scan-throughput`, `scan-throughput-*`.

### D26 Cancellation is `map_file`'s argument, polled per chunk or leader window by the mapping passes alone
A block can be hundreds of gigabytes, so block-boundary cancellation is a hang. A mapping pass (`map_file`,
`Table::settle`) takes a `tokio_util` `CancellationToken` as a required argument, polls it (D63) and keeps what it
banked; a query's streams take none, a dropped stream being their cancellation (D90). The preamble scan polls none: a
stop there would read as the first `COPY` header and cache as complete. `parse` dies by the signal it caught, so a shell
script stops (RT20), and a second signal ends it in the handler; as its namespace's init `pgdt` exits 128+n instead
(RT19). Rejected: a `ScanOptions` field (options are reused; a fired one stops every later run); a token type of our
own; `Option<&CancellationToken>` or a `Stop` trait inside the passes; `exit(128+n)` outside an init.

### D27 UTF-8 is validated once per chunk; `unsafe` is denied everywhere and allowed at the site that needs it
One SIMD validation of the largest line-terminated prefix per chunk, not one `from_utf8` per field, skipped where
nothing is decoded; the escaped path still validates (`\xNN` synthesizes bytes). In D68's seven crates the sites are
`append_view_unchecked` (`batch/chunks.rs`), its bounds from the `contains` that produced its coordinates and its
validity from the decode's borrowed arm (D46), and `LineMapping`'s system calls (D139); `pgdt-gzip-seek`'s zlib module is
the other shipped site (UF4). The workspace
denies `unsafe` in every target and each root forbids it unless a site of its production tree allows it
(`scripts/layering.py`). Four `unsafe` attempts lost to the safe shape.

### D28 One row split, shared unconditionally
One `RowSplit` per row, memoized and shared unconditionally by the filter's terms and the batcher, extending only as
deep as asked. Rejected: a term-count gate; a second `push_row` for the unfiltered path (a second `push_field` site
defeats inlining); stopping the walk at the last projected column, `push_row` being the only `ColumnCountMismatch` site.
Evidence: `predicate-terms`.

### D29 Refused micro-optimizations on the row path
Pre-sized Arrow builders (a one-row batch fills thousands of slots); a viewing builder for nested `Utf8View` (the prize
is zero below arrow's inlining width); skipping a block with `memmem` (the row count and the census read every row); a
proposal is sized against the per-row profile. Evidence: `nested-decode-micro`, `map-only`, `cross-file-floor`,
`nested-end-to-end`.
