# P13.3 — `XzSource`: the seek table's xz content, and the streaming decoder restarted on seek

## What landed

**`XzSource` is a new `ByteRangeSource` implementation in `io.rs`** (D5, D6),
backed by `xz_seek::Reader<std::fs::File>` behind a `std::sync::Mutex` — the
reader owns a single live decode and its current position and its own
`read_at` takes `&mut self`, where this trait's methods take `&self`. Nothing
calls a source concurrently today (every read loop in this crate is
sequential); D6 hands parallel decode to P16 explicitly, and the mutex is
what that phase will need to replace with its own scheme.

`XzSource::open(path)` opens two independent handles on the same file: one is
moved into the `Reader`, which walks the file's stream footers immediately
(`xz_seek::SeekTable::from_source`'s cost — one read per stream plus one for
the file's tail) and owns the handle for decoding from then on; the other is
kept untouched for `stored_size()`/`modified()`, which report the compressed
file's own on-disk facts with a plain `stat` and must never disturb the
decoder's live position to do it. `xz_seek::Reader::new` defaults to
`Verify::Full` on its own, so nothing here has to ask for it — D6's and D9's
"the bulk path's stronger default" is simply what the crate already does
without a configured `Builder`.

**The trait method map, read straight off `ByteRangeSource`'s existing
shape:**

- `size()` — the seek table's `uncompressed_size()`, already known from the
  walk at `open` time; no further I/O.
- `stored_size()` — a `stat` on the second handle: the compressed file's own
  on-disk length (D4), never the number `size()` answers.
- `size_is_exact()` — left at the trait's `true` default; D7 already says xz's
  own length is exact.
- `modified()` — the second handle's mtime, same shape as
  `LocalFileSource::modified`.
- `read_range(offset, len)` — `spawn_blocking` over
  `Arc<Mutex<Reader<File>>>::read_at`, pooled through the same `BufferPool`
  `LocalFileSource` uses (`XzSource` is defined in `io.rs` precisely so it can
  share that private type). `read_at` is fill-or-EOF; a short return is
  turned into `Error::Io(UnexpectedEof)` to match `LocalFileSource`'s
  `read_exact_at`-based contract — every read loop in this crate already
  clamps `len` to avoid hitting this, so a short read here means a
  caller/source disagreement, not a normal outcome.
- `hint_read_size(len)` — forwards to the shared pool, unchanged from
  `LocalFileSource`'s.

**A new `ByteRangeSource::seek_table()` method, defaulted `None`,** is what
lets `cache::save` (which only ever sees `&dyn ByteRangeSource`) ask a source
for the table to persist without knowing its concrete type.
`LocalFileSource` never overrides it; `XzSource` returns a clone of the table
it already built at `open` time — no re-walk. `cache::save` now builds
`compression: source.seek_table().map(CompressionIndex::Xz)` instead of the
hardcoded `None` 13.2 left in place, which is the whole reason
`CompressionIndex` existed a slice early: no `FORMAT_VERSION` bump was needed
for this.

**`Error::Xz(#[from] xz_seek::Error)`** joins the taxonomy — every fault
`xz_seek` raises (a bad walk, a failed block check, an unsupported check id,
…) reaches a caller through the same `pgdump_query::Error` every other
failure does, with `Display` forwarding to the wrapped error's own message.

**`XzSource` is exported** (`pub use io::{..., XzSource}` in `lib.rs`), so a
caller — today, only this phase's own tests — can construct one directly.
Recognition (sniffing the `.xz` magic and choosing between `LocalFileSource`
and `XzSource` automatically) is D8's job and lands with 13.4; this slice
does not wire either CLI call site to it.

## What was deliberately left for 13.4 and 13.5

- **No recognition.** `XzSource::open` takes a path and always treats it as
  `.xz`; nothing here sniffs magic bytes or offers a `LocalFileSource`/
  `XzSource`-choosing convenience. That is D8.
- **No non-seekable diagnostic.** A single-block file decodes correctly (this
  slice's own tests cover it — see below) but nothing announces the cost;
  `SeekTable::blocks_in(range)`, which D2's warning wants a number from,
  is not in the vendored snapshot (`54c7983`) yet either. Both are 13.4's.
- **No committed fixtures.** The phase's "Fixtures" section reserves the two
  `edge_cases.sql`-derived `.xz` files (seekable multi-block, non-seekable
  single-block) and the CLI-level differential parity against them for 13.5.
  This slice's own tests build their own throwaway payloads with `xz` shelled
  out from Rust, and a separate integration test in `tests/cache.rs` compares
  an `XzSource` over a freshly-compressed `edge_cases.sql` against the same
  file read plain — a library-level parity check, not the CLI-level one 13.5
  owes.
- **The persisted `compression` field is written, never read.** `cache::load`
  round-trips `Option<CompressionIndex>` like any other field, but nothing
  yet constructs an `XzSource` from a cached table via
  `xz_seek::Builder::open_with_table` to skip the footer walk on a cache hit.
  That is naturally 13.4/P14's territory (recognition needs to decide *which*
  source to build before it can decide whether to skip the walk), not
  something this slice's own scope commits to.

## Testing

**Unit tests, in `io.rs`'s own `#[cfg(test)]` module** — `XzSource`'s wiring
in isolation, against throwaway payloads compressed with `xz` shelled out
from the test itself (`require_xz` asserts the binary is runnable rather than
skipping, per `docs/design/roadmap.md`'s "A test may assume the tools `mise`
pins" — `xz` is the one exception that rule already names, since `mise`
cannot pin it): `size()`/`stored_size()` disagree correctly, `seek_table()`
reports every block, forward reads spanning several blocks match the
plaintext, a backward read across a block boundary restarts and still
matches, and the non-seekable (single-block) shape decodes correctly forward
and backward.

**One integration test, in `tests/cache.rs`** — `build_index` over an
`XzSource` wrapping a freshly-compressed `edge_cases.sql` produces the exact
same `DumpIndex` as `LocalFileSource` over the plain file, and a
`cache::save`/`load` round trip through the `XzSource` reloads as `Valid`
with the plain file's own `total_size`. This is the differential parity this
slice owes at the library level; the CLI-level parity against the phase's own
committed fixtures is 13.5's.

`cargo test --workspace`, `cargo clippy --workspace --all-targets` and
`cargo fmt --check` all pass with no new warnings.

## What this changes for measurement

Touches `io.rs`, `cache.rs`, `error.rs` and `lib.rs`. `io.rs` and `cache.rs`
were already declared paths for every figure `measure.py --stale` reports red
(from 13.1's/13.2's changes); this slice adds no figure to the stale set that
was not already there, and no number in `measurements.md` is touched or
contradicted. No sweep is owed, per the standing rule that a stale figure
does not oblige one.

## What 13.4 inherits

- `XzSource::open(path)` is a working, tested `ByteRangeSource` today —
  13.4's recognition convenience only has to decide *when* to call it, not
  build anything new underneath.
- `ByteRangeSource::seek_table()` is the hook a future `open_with_table` path
  reads from: recognition (or a cache-aware opener above it) can check a
  loaded cache's `compression` field and, on a hit, hand the table to
  `xz_seek::Builder::open_with_table` instead of paying `XzSource::open`'s
  walk again. Nothing does this yet.
- D2's diagnostic still needs `SeekTable::blocks_in(range)`, which is not in
  the vendored `xz-seek` snapshot (`54c7983`). 13.4 either re-syncs the vendor
  copy first (if that method has since landed upstream) or names the block
  count some other way — `SeekTable::block_count()` already exists and is
  what this slice's own tests use to confirm a file's shape.
- `Error::Xz` is in place, so 13.4's diagnostic and any error-path tests it
  adds have a variant to match on rather than needing to introduce one.
