# P16.12 — The determinism test

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md). How the mechanism works is
[`architecture.md`](architecture.md), "The interior split" — in particular "The
mapping pass is the leader" — and what the test does is beside the rest of the
suite, in that doc's "Testing philosophy".

## What exists now

**`pgdump_query-cli/tests/determinism.rs`.** Two tests, no library code, no new
public surface:

- `every_fixture_parses_to_the_same_cache_at_every_stated_parallelism` — all
  109 generated fixtures, each parsed six times, the five legs compared byte for
  byte against a `--jobs 1` reference: flagless, `--jobs 8`,
  `--jobs 1 --chunk-size 512`, `--jobs 8 --chunk-size 512`,
  `--jobs 8 --chunk-size 4096`.
- `a_region_past_the_shipped_chunk_size_writes_the_serial_cache` — one
  generated dump whose `COPY` region clears four shipped chunks, parsed at
  `--jobs` 1/4/8/24 with **no** chunk size stated.

`common::all_fixtures()` is new in the CLI's test module, a deliberate second
copy of the library test module's function of the same name: the two test
crates cannot share a module, which is what both `common/mod.rs` files exist to
say.

The pair runs in **4.8 s** on this machine, 654 process spawns of the debug
binary over 1.4 MB of fixtures plus one 4 MiB generated dump.

## The calls worth knowing about

**The parallel legs state a chunk size, and dropping it would hollow the test
out silently.** `LocalFileSource`'s partition unit is the read chunk and
`leader::scan_region` declines a region with less than one partition left in the
file, so at the shipped 1 MiB every fixture — the largest is 66 KB — is declined
whole. A `--jobs 8` leg without `--chunk-size` is therefore the serial path
compared to itself: green, fast, and asserting nothing. It is kept in the list
anyway, as the leg that says the flag alone changes nothing; the cutting is
bought by the 512-byte and 4 KiB legs beside it, which is the same pair
`map_file.rs` uses and for the same reason.

**The generated dump is what covers the shipped configuration.** Its
precondition — four chunks of data past the region's data offset — is
**asserted against `DEFAULT_CHUNK_SIZE`** rather than assumed, so a raised
default fails the test naming the numbers instead of quietly turning that test
into a sixth serial comparison. That is the same discipline `xz_source.rs`
learned when a `--block-size` that never split anything was labelled
"seekable".

**Both assertions were shown to fail on injected breakage first.**
`leader::merge`'s fold was temporarily changed to `row_count += piece.rows + 1`.
Both tests fail; in the sweep the first failing leg is
`--jobs 8 --chunk-size 512` on the very first fixture, and the flagless and
`--jobs 8` legs pass — which is the sweep confirming from the outside both that
the small-chunk legs reach the leader and that the shipped-chunk ones do not.

**Each run gets a cache path of its own.** The library refuses to overwrite a
cache recorded against a different file (`Error::CacheSourceMismatch`), so a
reused path fails the second fixture with an error rather than asserting
anything about the first. Found the direct way: the first sweep written this way
failed on fixture two of 109.

**Fixtures are read where they lie, not copied into a tempdir.** The cache
records the source's stored size and mtime, so two legs looking at two copies
would compare two different identities and the test would fail on a field that
has nothing to do with `--jobs`. Only the cache path moves.

**Bytes, not the index.** `map_file.rs` already compares the in-memory
`DumpIndex` a parallel mapping pass builds against an eager one's. What this
adds is the encoding: a field that compares equal and serialises differently
would leave `pgdq info` reporting one thing after a serial scan and another
after a parallel one, with nothing in the library's suite able to see it.

## What the next slice inherits

**Nothing here licenses a number.** Every leg is a debug binary on a
one-megabyte tree; no timing from this file may be quoted, and `16.13` is where
the parallel scan acquires a price.

**The sweep is discovery-driven**, so a schema or flag set the generator gains
is covered the moment it is written — and a fixture that stops parsing fails
here as well as in `map.rs`'s tiling sweep.

**`16.14` is the same claim at real scale**, and it is the one this file cannot
make: 109 kilobyte-scale fixtures say nothing about a block that spans dozens of
windows, a cut that lands inside a multi-gigabyte region, or a resumed scan
crossing one. What covers that is a serial and a `--jobs 4` parse of koji's
`.xz` taken in one run and compared to each other — not, as this sentence
originally said, a comparison against the serial 784 GB plain scan's cache,
which is an artifact nothing produces
([`roadmap-P16.14-koji-verification-notes.md`](roadmap-P16.14-koji-verification-notes.md)).

**`.xz` determinism is not asserted here.** The fixture tree is plain `.sql`;
`xz_source.rs` carries the compressed source's end-to-end parity and
`parallelism.rs` its budget parity, both on rows rather than on cache bytes. A
compressed `parse` at two job counts writing one cache is asserted only by
`16.14`, once, on a file nobody can rescan cheaply — nothing in the suite makes
it, and it is cheap to add wherever a slice next touches that path.

## Figures

**No figure goes red.** The change touches
`pgdump_query-cli/tests/determinism.rs`, `pgdump_query-cli/tests/common/mod.rs`
and `docs/design/architecture.md`; no figure declares a test path, and
`architecture.md` is a `quoted_by` edge rather than a `depends` one — what a
figure invalidates, not what invalidates it. `uv run measure.py --stale` reports
the same seventeen figures, from the same paths, as before this slice.
