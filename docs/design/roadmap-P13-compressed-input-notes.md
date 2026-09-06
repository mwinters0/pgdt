# P13 — Compressed input: what the wrap left over

**This doc is short by design, and that is the wrap working rather than the
wrap skipped.** `architecture.md` exists and is filed by subject, so everything
P13 built and every alternative it refused went *there* — "The compressed
source" for the byte source, recognition, the shapes and the vendored decoder,
"The cache" for the identity split and the compression-index envelope field,
"Diagnostics: one severity scale, two types" for the non-seekable warning,
"Execution model and API surface" for the dyn-compatible trait and the three
defaulted methods added for sources the local file is not, and "Testing
philosophy" for the differential-parity suite. Consolidating the five slice
notes verbatim would have rebuilt the second authority the keystone was run to
remove. What is below is the residue: things that are true of the *phase*
rather than of a mechanism, and things a later phase needs that no inbox already
carries.

## The spec promised one thing the slices did not deliver, and it is now `KD15`

D1 says the seek table is "persisted in the cache, so only a source with no
usable cache ever pays for" the footer walk. Half of that shipped: the table is
persisted, `cache::load` round trips it, and `pgdq info --dqcache <path>` with
no `--source` opens no source and so genuinely never pays. The other half did
not — `XzSource::open` always walks the footers, and nothing constructs a reader
from a cached table. No slice row committed to that half, so no slice is
unfinished; the gap is between D1's prose and what the five rows between them
covered. It is `KD15`, unowned, detailed beside the mechanism
([`architecture.md`](architecture.md), "The compressed source"), and the
reasoning is [`../status/history/2026-09-06.md`](../status/history/2026-09-06.md),
"P13's wrap audit: the seek table is persisted and never read back".

## The vendor copy was deliberately not re-synced, and that is P16's to undo

`SeekTable::blocks_in(range)` — asked of `xz-seek` on this project's behalf,
specifically so the non-seekable warning could carry a block count scoped to a
query's touched byte range — landed upstream *after* the snapshot this repo
vendored (`54c7983`), as part of that crate's own parallel-block-decode work.
The re-sync was available and was refused: the snapshot was taken when it was
precisely to sit still while that work moved the source underneath it, and the
warning D2 actually asks for is file-wide, which `SeekTable::block_count()`
answers from the frozen copy. So the diagnostic reports a whole-file count
(always 0 or 1 once `is_seekable()` is false), and **the first consumer that
needs post-`54c7983` upstream work is what re-syncs the copy** — which is P16,
whose whole interest is that parallel decode.

Read this beside the entries already in P16's inbox about the decode pool and
its pieces: those describe what the crate will ship, and this describes why the
vendored copy in *this* tree does not have it yet.

## No invariants-register entry is owed, and the reason is the register's ritual

[`postgres-invariants.md`](postgres-invariants.md) is scoped to properties of
`pg_dump`'s output — or of PostgreSQL itself where a decision turns on what the
server accepts — and its whole payoff is one ritual: when a new PostgreSQL major
lands, walk the file and re-run every entry's re-verification step. P13's
external dependencies are the xz container format and the vendored decoder,
neither of which a PostgreSQL release can invalidate, so an entry for them would
be read at a trigger that has nothing to do with them. The container facts the
design leans on — the stream index carrying every block's compressed and
uncompressed size, the six-byte magic, the three shapes and what each costs —
are stated beside the mechanism instead ([`architecture.md`](architecture.md),
"The compressed source"), and the decoder's own guarantees are that crate's
register to keep.

## Two small negative results with nowhere else to sit

**`Arc<dyn ByteRangeSource>` was named in the trait rework and unexercised by
it.** Making the trait dyn-compatible converted every generic consumer to
`&dyn ByteRangeSource`, and no existing call site needed an owned handle —
every one of them held a plain reference. The owned form only earned its keep
two slices later, when recognition had to *return* a source whose type is
decided at run time, which is what the CLI now holds. Worth knowing when a
similar rework is costed: the borrow form covers the library, and the owned form
is what a factory function forces.

**Exporting recognition at the crate root, not through a public `io` module.**
The spec sketched `pgdump_query::io::open_local` and flagged the name as
unsettled. `mod io` was already private with its three types re-exported at the
crate root, so the function joined that list rather than making the module
public — the privacy boundary is unchanged and the function is reached the way
its neighbours already are.

## The phase owes no measurement row, and the wrap does not create one

Deliberate, and it survives the wrap: the number a caller actually wants is
concurrent decode throughput against the plain path's device-bound figures, and
that is unreachable until parallel decode exists. The decode readings this phase
rested on are **probes** — no harness, no `drop_caches` discipline, no
`measure.py` registration — and no document quotes them as measurements.
`measure.py --list` gains no row here. What the phase *did* leave behind is a
stale register: reshaping every `ByteRangeSource` signature touches the hot read
path, and there is no mechanical oracle for a library change, so those figures
stay red until a sweep and a stale figure obliges none
([`../status/STATUS.md`](../status/STATUS.md);
[`measurements.md`](measurements.md), "A stale figure does not oblige a sweep").
