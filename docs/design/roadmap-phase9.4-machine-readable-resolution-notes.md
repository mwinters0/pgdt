# 9.4 — Machine-readable resolution

`pgdq info --json` now carries per-`COPY`-block type resolution — the
per-column outcomes `--verbose` computed and threw away at the JSON boundary.

## The shape

`IndexJson.resolution` is one `BlockResolutionJson` per block, in file order,
each `{ database, table, header_offset, columns }`, each column
`{ name, declared, outcome, arrow_type, plan }`.

- `outcome` is a stable snake_case token (`mapped`, `varying_array_shape`,
  `metadata_not_scanned`, …).
- `arrow_type` is the **exact string** `--verbose` prints for that column, from
  the same `arrow_type_label`.
- `plan` is the `NestedPlan`, serialized structurally. `pgtype::NestedPlan`
  gained `Serialize` (not `Deserialize`) for this. It is the one thing the
  Arrow type cannot say — `int4range[]` and `int4multirange` share a
  `DataType` — and emitting it structurally beat inventing a second spelling
  for a vocabulary that already exists. **It is still never persisted**:
  `layering.md` rule 5 keeps L2 conclusions out of the cache, and nothing in
  `CacheFile` reaches it.

A header-less block is exported with an empty `columns` list rather than
omitted, so the document's shape does not vary per block. Same reasoning as
`MetadataNotScanned` being a value rather than an absent key.

## One resolution pass, two renderings

`block_resolutions(index, complete) -> Vec<(&CopyBlock, ResolvedSchema)>` is
the single pass. `print_index` and `print_index_json` both consume it;
`print_index` no longer calls `resolve_columns` itself. A second implementation
was the failure mode this slice had to avoid — the export would drift into
describing a different vocabulary from the listing, and nothing would catch it.

`resolution_words(&ColumnResolution) -> (token, sentence)` is the other half:
**one exhaustive match returning both spellings**, so a new variant cannot be
given a token without a sentence or vice versa. `resolution_label` is now a
thin `.1` accessor over it.

The cross-check is
`partial_reporting.rs::the_json_export_and_the_verbose_listing_agree_column_for_column`,
which reconstructs every expected `--verbose` line from the JSON and finds it
in the text. It exploits a property worth keeping: **every sentence begins with
its token's words**, `_` replaced by spaces. Break that and the test says so.

## `ColumnResolution::MetadataNotScanned`

The seventh variant. `resolve_columns` produces it, in `SchemaMode::Typed`
only, when `metadata` is `Some` but holds no `preamble_complete` entry for the
block's database — a `pg_dumpall`/`--create` dump whose scan stopped inside a
later database. `metadata: None` stays `NotDeclared`: a caller with no DDL at
all has no scan to finish.

Two things are deliberately *not* symmetric here, and the asymmetry is the
point:

- **The streaming path still refuses.** `stream::resolve_block` raises
  `Error::MetadataNotScanned` before the schema is built, because a stream
  hands back rows and a wrongly-typed one is a wrong answer with no signal.
  That check is unchanged and runs *before* `resolve_columns`, so the new
  variant is unreachable from a query.
- **The reporting path degrades and names the reason.** A listing covers every
  block in the index; one unresolvable block must not sink the document.

*Not* reusing `NotDeclared` is the whole reason the variant exists:
`NotDeclared` means the dump never explained this column and is final, this one
means finish the parse and ask again — identical-looking output, opposite
advice.

## The fixture rule's one legitimate exemption

`tests/pgtype.rs::every_resolution_outcome_is_produced_by_a_real_fixture_column`
requires a real fixture column behind every `ColumnResolution` variant.
`MetadataNotScanned` is left out of `EVERY_OUTCOME` and listed in
`outcome_name`'s exhaustive match with the reason inline. It is a property of
*how much of the file was read*, not of a declared type, and every fixture
there is scanned to EOF — so no fixture can produce it, which is exactly the
exemption that test's doc comment already anticipated. The outcome is covered
against a real truncated index in
`pgdump_query-cli/tests/partial_reporting.rs` instead, both directions: the
later database's blocks report it, and the same index resolves them properly
once the parse finishes.
