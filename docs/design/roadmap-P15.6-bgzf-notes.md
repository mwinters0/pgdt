# P15.6 notes — BGZF

A BGZF file's index is walked off its members' headers and trailers at open,
on both arms, with nothing decoded; every read after the walk is the indexed
source's, parallel at units of whole members.

- **`source/compressed/deflate/walk.rs` is the walk**: `walk_file` pulls from
  the file, `walk_fetched` fetches through the transport, both driving
  `gz_seek::Walk` through one `Walking`, each read run on to `WALK_CHUNK`
  past the walk's request so a read covers many members. Members are
  coalesced into units of at least `CHECKPOINT_SPACING`, every unit starting
  at a member and carrying no window, so the codec opens over it with no
  provider.
- **Recognition reads 18 bytes** (`RECOGNITION_BYTES`, which
  `ORIGIN_LEADING_BYTES` is): gzip's magic still names the format, and
  `is_bgzf` over the first member's `BC` subfield chooses, in `open`'s
  gzip arm on each opener, the walk over the first pass. `Format` is still
  `{Xz, Gzip}`: the index a walk builds is the one a first pass builds, so a
  cached BGZF index is checked against gzip's magic like any other.
- **A chain that breaks hands the file to a first pass**: a first member
  BGZF's and a later one not, a length running past the file, an `ISIZE`
  past 64 KiB. The walk warns, naming the cause, and the first pass then
  reads a gzip file of several members, or refuses a damaged one where it
  meets the damage (D18). So no decision depends on a file keeping BGZF's
  rules, and no invariant is filed: what they buy is a walk, and a unit
  decode checks each member's trailer either way.
- **`CompressedFileSource::open` walks a BGZF file**, and still refuses any
  other gzip file by name. `walked(path, format)` became `xz(path)` and
  `bgzf(path, named)`, the second `None` where the chain breaks.
- **The member count is the walk's**: `gz_seek::End` carries `members`,
  `Index::member_count` answers it for a complete index and counts member
  checkpoints for an incomplete one, and `from_parts` refuses an end counting
  fewer members than the checkpoints begin. `GzipIndex::member_count` reads
  it, so `info --detail`, `--json`'s `streams` and the first pass's complete
  line count every member. `CACHE_FORMAT_VERSION` is 76, the gzip index's
  persisted end gaining the count; plain and `.xz` caches encode as at 75.
- **Announced as the `.xz` walk is** (D64): `BGZF member walk started` and
  `… complete` with `units` and `members`, `fetches` too on the fetched arm,
  each naming `source` and `codec=gzip`, at `info`; the broken chain at
  `warn`. Under "Decisions worth another look": the fetched walk is not
  warned of as the fetched `.xz` walk is.
- **The non-seekable gzip warning names `bgzip`** beside `gzip` and a level
  above 1, which 15.5.3 left for this slice.
- **The evidence**: `deflate/walk/tests.rs` (both arms walking one index,
  units of the spacing, every member counted, every unit read back; one read
  a chunk; a mixed and a cut file left to a first pass, which reads the mixed
  one whole; `open` and `CompressedFileSource::open` walking, a single member
  refused by the latter); `gz-seek`'s `bgzf/tests.rs` and `index/tests.rs`
  (the count, a forward pass agreeing with it, an end counting fewer refused);
  `crates/pgdt/tests/gzip_source.rs`'s
  `a_bgzf_dump_is_walked_at_open_and_reads_as_its_plaintext` (cold and warm
  reads equal to the plaintext's, one started and one complete line on a cold
  `parse` and no first pass, none over the cache, every member in `info`);
  `determinism.rs`'s `a_bgzf_dump_writes_the_serial_cache_at_every_stated_parallelism`
  (three units, byte-identical caches at the recommended count, 8 workers and
  a piecewise budget); `remote.rs`'s
  `a_remote_bgzf_file_is_walked_at_open_and_read_by_its_index`.

## What later slices inherit

- **15.8 is a single member's, and a fetched BGZF file's**: a local BGZF
  file's units are known at open, so nothing of a parallel first pass touches
  it; a fetched one is a first pass's from 15.13 on, its units taken from its
  members' headers in 15.8; a broken chain's first pass is 15.8's like any
  other.
- **15.7's manual** writes the walk's two lines into `dump-inspection.md`'s
  "Status on stderr" beside the `.xz` walk's; this slice corrected only the
  sentence saying a gzip file names no place a read could begin.

## Negative results

- **A `Format::Bgzf` was not added**: BGZF changes how a gzip file's index is
  found, not the format a cached index is checked against, so a variant would
  need a mapping back to gzip at every comparison.
- **The probe's leading bytes are not handed to the walk**, as the fetched
  `.xz` walk's are: the walk's first read, run on to a chunk, covers them,
  so they would save no fetch.
