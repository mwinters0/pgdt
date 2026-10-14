# P36.2.1 notes — the seek table opaque

`pgdt_io::SeekTable` (`source/xz/table.rs`) wraps `xz_seek`'s table. Its
rustdoc says what an embedder reads of it. `KnownCompression::Xz`,
`ByteRangeSource::seek_table`, both sources' `with_table` and `pgdt-dump`'s
`CompressionIndex::Xz` all take or return it, so no API names `xz_seek`.

## What landed beside the spec's words

- **The persisted form is unchanged, so there is no cache bump.** Under
  `pgdt-io`'s new `serde` feature the type serializes `transparent`, which
  is byte for byte `xz_seek`'s own table, in bincode and in `info --json`'s
  `seek_table`. Two tests pin this, and both pass unedited:
  `persisted_index_is_pinned_to_the_format_version` in
  `pgdt-query/tests/cache.rs`, and `seekable_xz_parses_to_the_same_index_as_plain`
  in `pgdt/tests/xz_source.rs`. `pgdt-dump` turns the feature on and no
  longer depends on `xz-seek` at all.
- **`walk_seek_table` is `pub(crate)`, not `internal`.** The checklist row
  said `internal`, but nothing above `pgdt-io` calls it, and D106 gives
  `internal` only what a crate above reaches. Only the fetched path walks,
  so it compiles under `http` or `test` alone.
- **The API gives the plaintext's shape and nothing compressed**: block and
  stream counts, seekability, the largest block, the two sizes and each
  block's plaintext range. Those are what `pgdt`'s embedder tests read.
  `pgdt-query`'s one test that needs a block's compressed extent, to corrupt
  its check, walks the file with `xz_seek` itself, which its tests already
  depend on.

## What the next slices inherit

- **`xz_seek::Error` still reaches an embedder** through
  `Error::xz_cause::<E>()`, the only way to tell one `.xz` failure from
  another, such as `InvalidTable` or `BlockCheckFailed`. The spec's cluster I
  names only the seek table, so this slice left it alone. 36.10, which puts
  errors in library terms, is where it would be weighed.
