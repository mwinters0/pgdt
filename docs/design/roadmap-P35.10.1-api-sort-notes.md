# P35.10.1 notes — each crate's API sorted by its purpose

What 35.10.2 and 35.5 inherit from sorting each crate's public surface by
the spec's definition ([`roadmap-P35-seams.md`](roadmap-P35-seams.md), "What
this phase does not do") instead of 35.10's proxy, "public before the carve".
Each crate's `lib.rs` is where it lands. No body changed and no test moved;
`CACHE_FORMAT_VERSION` stands.

## How the sort fell

- **Three tests, in order.** An item an embedder of the crate would call,
  given what its `lib.rs` says the crate is for, is API. So is anything an
  API item's signature, public field or variant names, since an API reached
  only through `internal` is no API: `TextGrammar` and `Geometric` ride
  `NestedCompare::Uncomparable`, `Unchecked` rides `UncheckedColumn`,
  `Partitioning`, `WaitPolicy` and their kin ride `ByteRangeSource`,
  `CacheLoad`, `WeakIdentity` and `OriginMatch` ride the cache's loads. And
  `Truth` stays API with nothing returning it, being the domain `Expr`,
  `Predicate` and `PredicateOp` document their answers in.
- **What moved to `internal`**:
  - `pgdt-codec`: `char_typmod_refuses`, `numeric_in_stores`,
    `NUMERIC_DSCALE_MAX` and `NUMERIC_INTEGER_DIGITS_MAX`, out of `decode`.
    They answer whether PostgreSQL refuses a field, not what it reads as,
    beside `grammar_refuses` and `field_refused`, which 35.10 already put
    there.
  - `pgdt-query`: the per-block statistics re-read — `gather_block_statistics`,
    `BlockReread`, `bounded_columns`, `StatisticsBackfill`, and
    `StatisticsRequest::backfill` behind `StatisticsRequestExt` — and
    `pgdt-model`'s `BlockGathered`. That was wrong: P10's spec designed the
    re-read as the library's per-block back-fill entry point, its result
    stored through `pgdt-model` API, and 35.10.3 restores it
    ([`../status/history/2026-10-10.md`](../status/history/2026-10-10.md),
    "The per-block re-read, reviewed").
- **One path**: `pgtype`'s seventeen items left `pgdt-codec`'s root, so they
  are `pgdt_codec::pgtype::<item>` alone. Every other crate already had one
  path per item.
- **What stayed, by purpose**: `pgdt-core`'s whole `COPY` text format, as the
  spec names it; `pgdt-model`'s vocabulary whole, every type being data a
  `DumpIndex` carries or a constant bounding it; `pgdt-io` whole —
  `walk_seek_table` is what builds the table `FetchedXzSource::with_table`
  takes; `pgdt-dump`'s scanner with `ChunkCarry`, the protocol `CopyScanner`
  documents for driving it, and the map builder's `attach_text`,
  `check_tiling` and `dump_metadata_from_spans`; `pgdt-codec`'s nested
  literal codec at its root, as the spec names it, and `decode`'s decoders,
  decimal digit readers included.
- **Content `lib.rs` lists that is not API**: `pgdt-core`'s SQL lexer,
  plumbing for the scanner and the preamble, and `pgdt-codec`'s datetime
  grammar, a crate-private refusal check. The spec's test is what a crate is
  for, not what its `lib.rs` lists. The sort left `ValueKey` in `internal`
  too, which was wrong: it already hides `OrderKey` behind `of` and an order
  over API inputs, so 35.10.3 makes it API. `NestedKey` has no such wrapper,
  so ordering a nested value stays internal, `KD116`
  ([`../status/history/2026-10-10.md`](../status/history/2026-10-10.md),
  "The codec's ordering keys, reviewed").

## What 35.10.2 and 35.5 inherit

- **`decode`'s non-API items now include the four refusal checks**, for
  35.10.2's private child module at the file's seam; `pgtype`'s are 35.10's.
- **The facade re-exports `pgtype`'s items from `pgdt_codec::pgtype`**, and
  dropped the re-read's five names, which 35.10.3 restores.
- **35.5's check holds of this tree**: no item sits at a crate's root and in
  a namespace, nor in both API and `internal`, and no embedder names
  `pgdt_query::internal`.

## Negative results

- **"Named by no embedder" sorts almost nothing out**: most of the ~130
  exports no embedder names are API by purpose or by an API signature, and
  the sort moved ten.
