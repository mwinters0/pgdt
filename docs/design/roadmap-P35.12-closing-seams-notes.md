# P35.12 notes — the closing seams round

The round over the tree the cuts from 35.13 to 35.26 left. Its report named
no file past a hard limit and one past a soft one; this doc holds that file's
cut and what the read of every crate for the `seams` skill's "Antipatterns"
found. No behaviour changed and no test changed what it asserts; nothing
persisted moved, so `CACHE_FORMAT_VERSION` stands.

## The report

`uv run file_limits.py` at `147fa792`: none past a hard limit;
`pgdt-codec`'s `value/nested.rs` past the soft function limit; the
`datetime/grammar.rs` port exempt. `module_graph.py`: one cycle, the accepted
`origin` ↔ `remote`. The full reading and the doc links against the edges are
`runs/seams-closing-20261011/`.

## The nested literal, its two scanners apart

`value/nested.rs` held D45's two scanners one above the other under a banner,
each reading the three literal types and nothing of the other. They are two
concerns by the decision that keeps them apart, so each is a child, and the
parent holds what both produce:

```
value/nested.rs          the nested literal: ArrayLiteral, RecordLiteral, RangeLiteral, and the whitespace all four grammars agree on; pub use of each scanner's API
  nested/output.rs       what a dump holds: the *_out forms read and written — Syntax and its three instances, the quoted-token scanner, decode_*, render_*, push_array_null, quote_array_element
  nested/input.rs        what a user typed: the *_in supersets — parse_*
  nested/key.rs          unchanged
```

```moves
pgdt-codec::value::nested -> pgdt-codec::value::nested::output
pgdt-codec::value::nested::ArrayLiteral -> pgdt-codec::value::nested
pgdt-codec::value::nested::Elements -> pgdt-codec::value::nested
pgdt-codec::value::nested::RecordLiteral -> pgdt-codec::value::nested
pgdt-codec::value::nested::RangeLiteral -> pgdt-codec::value::nested
pgdt-codec::value::nested::is_space -> pgdt-codec::value::nested
pgdt-codec::value::nested::MAXDIM -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::ArrayToken -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_array_token -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_quoted_array_element -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_unquoted_array_element -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_array_body -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_dimension_int -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::skip_space -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_array -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_array_delimited -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_quoted_run -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_record -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_range -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::read_range_bound -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::MultirangeState -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_multirange -> pgdt-codec::value::nested::input
pgdt-codec::value::nested::parse_int2vector -> pgdt-codec::value::nested::input
```

The tests split with it: `output/tests.rs` round-trips what a dump holds,
`input/tests.rs` holds the server-taken `*_in` corners and the check that
every output form parses as itself. `measure.py` declares both children
beside the parent, under `NESTED` and the `nested-decode-micro` figure's
`depends`, so a cut does not hide a change from the figures that price it.
**Kept:** both scanners read the parent's private `is_space` and `Elements`,
a child reaching up; the whitespace is the one predicate all four grammars
agree on (I20, I44) and the alias is `ArrayLiteral`'s field, so both are
the literal's, not either scanner's.

## The read, and what it removed

Every crate was read against "Antipatterns", each crate's review, with its
evidence and verdicts, in `runs/seams-closing-20261011/reviews/`. What no reviewer would dispute, and what
the design already implied, was landed — bodies unmoved but for whole moves:

- **Visibility widened and never narrowed back**, item by item, in pgdt-io's
  `source/`, pgdt-codec, pgdt-mapping's `gather/column.rs` and `interior`,
  pgdt-query's predicate and stream, datafusion-pgdump's `exec` and
  `filter/dynamic`, and `pgdt`'s `query/filter/`. pgdt-model's
  `ArrayShapeExt`, which no crate above called once the census fold moved
  into `block.rs`, is an inherent method again, and leaves `internal`.
  pgdt-io's `source/` narrowed its fields alone: `local`, `xz`, `blocks`,
  `BlockCache`, `XzBudget`, `BufferPool` and two constants stay
  `pub(crate)`, since `memory.rs`, `parallelism.rs` and `partition.rs` link
  them by path and a link resolves only where its path is visible.
- **Tests beside what they test**: pgdt-io's lone `xz/tests.rs` test into
  `xz/file/tests.rs`; pgdt-dump's `ddl/tests.rs` split into `ddl/table/` and
  `ddl/types/`, keeping the statement's own tests and the `parse_*` helpers
  both read, which sibling test modules cannot name in each other; and its
  tests reading only hand-built `DatabaseMetadata` moved to pgdt-model's
  `catalog/tests.rs`.
- **Stale docs**: module and crate docs scoped narrower than their modules,
  a citation by a file a cut moved, links to paths a cut moved.
- **Two helpers home**: pgdt-query's `null_refused` and `Spellings::matches`,
  read by evaluation alone, into `predicate/eval.rs`, as the 35.11 rule for
  `Lookup` already placed `contains`.

## What is not cut: the design questions

Each is a judgement — a new module or name, a moved concept, a merged
duplicate, a crate's part — so unattended it waits for the maintainer
(STATUS, "Decisions worth another look"), whose ruling cuts it, keeps it with
its reason beside the code, or files it in the seam register
([`../status/seam-findings.md`](../status/seam-findings.md)). Each names the evidence; the
remedy the reviewer drew is in its crate's review.

pgdt-core
- **B1** `instrument/` is cut by visibility: `hooks.rs` holds what `internal`
  exports of two concerns, statistics attribution and evaluation timing,
  whose rest is `instrument.rs`'s private `enabled` and `evaluation`, all six
  of the crate's `pub(super)` items serving it. A child a concern
  (`statistics`, `evaluation`, `decode`) is the remedy; 35.11 kept the tree.
- **B2** The SQL identifier rule, `Cursor`, lives in `copy.rs` and is read by
  `type_name.rs`, pgdt-dump's `ddl/` and pgdt-codec's `compare`, whose
  `collation_parts` re-reads a qualified name: an `identifier` module.
- **B3** `FieldRanges`, `field_ranges`, `COPY_TEXT_DELIMITER` and
  `COPY_BLOCK_TERMINATOR` are root API nothing outside the crate reads.

pgdt-model
- **B4** The group-density readings — `nth_smallest`, `min_rows_group`,
  `max_rows_group`, `BlockStatisticsExt`'s `breaks_max_rows` and
  `predicted_group_size` — persist nothing and are read by pgdt-mapping
  alone, beside `density_merges`: a concept's part in the wrong crate.

pgdt-io
- **B5** `read_by_blocks` slices a decoded block identically in `file.rs`
  and `fetched.rs`, only the fetch differing.
- **B6** The compression verdict is written twice, `open_local` and
  `open_remote`.
- **B7** `blocks/tests.rs` holds a test of `partition.rs`'s `cut`, the only
  reason `BOUNDARIED_PARTITION_UNITS` is crate-wide.

pgdt-dump
- **B8** `map.rs`'s `classify` turns a statement into a `SpanBody` from
  `ddl`'s items alone, seven of them `pub(crate)` for it and naming it in
  their docs: the grammar's, in `ddl.rs`.
- **B9** The column grammar sits in `ddl/table.rs` and `ddl/types.rs` reads
  five of its items: a `ddl/column.rs`.
- **B10** `lex` names two modules, `crate::lex` and `crate::ddl::lex`.
- **B11** `lex.rs`'s `strip_word` and `ddl/lex.rs`'s `strip_kw` are one rule.
- **B12** The cache file's writing half is in `store.rs`'s `save`, its
  reading half in `file.rs`, and pgdt-query's `tests/cache.rs` copies the
  writer.
- **B13** `ScanOptions` carries options only pgdt-mapping reads
  (`statistics_allowance_bytes`, `postgres_invalid_values`), which D103's
  wording places there.

pgdt-codec
- **B14** `macaddr`'s reader and renderer, and `timetz`'s reader, sit in
  `key.rs` while their classifiers sit in their families; 35.11 placed every
  `OrderKey` builder in `key.rs`, but `render_macaddr` builds none.
- **B15** `grammar/form.rs` writes what a filter's literal must look like,
  not the strict parse: `key/form.rs`, or `grammar` renamed to a type's input
  grammar.
- **B16** `value/libc.rs` holds `HEX_NIBBLE` and `strtoint`, neither the C
  library's.
- **B17** `declared.rs`'s one unit test is in `pgtype/tests.rs`.
- **B18** `ComparisonPlan::gathers_bounds` is API no caller reads.

pgdt-mapping
- **B19** Three half-splits forcing `gather/`'s `pub(super)` fields:
  `BoundsGatherer`'s heap and append read field by field in `column.rs`;
  `RowOrder`'s bytewise placement in `bounds.rs`, its keyed one in
  `order.rs`; the first group's join in `gatherer.rs`, every other column
  operation being `ColumnGatherer`'s. Each wants a method beside its type.
- **B20** `bounded_columns` twice: `backfill.rs`'s API forwards to
  `gather.rs`'s, which has no other caller.
- **B21** `announce_read_loop` sits in `backfill.rs` while `map_forward` and
  pgdt-dump's `scan` inline the same four hints.
- **B22** `internal::StatisticsAccount` is named by no crate.
- **B23** `gather/tests.rs` tests seven children, the only reason
  `text_upper`, `bytea_upper`, `BYTEA_ESCAPE_HEAD_BYTES` and
  `density_merges` are `pub(super)`.
- **B24** `interior/region.rs` reaches its parent's private `BlockEndings`,
  `empty_census` and `PieceObserver`'s fields; `BlockEndings` has no reader
  in the parent.
- **B25** `report_shortfall` sits in `forward.rs`, `Shortfall` in
  `interior/region.rs`, the same paragraph in both docs.

pgdt-query
- **B26** D78's believed check is written in `prune.rs` and `summary.rs`;
  what statistics are stored against is pgdt-mapping's `stored.rs`.
- **B27** `truths.rs`'s `required_ordering_terms` is a fact about rows,
  read by `prune.rs`, and re-flattens what `eval.rs`'s `conjuncts` does.
- **B28** A term's divergences are derived in `resolve_term`'s arms and
  again per column in `notes::postgres_divergences`, one oracle test holding
  them to one answer — which may be the point of two derivations.
- **B29** `predicate/tests.rs` drives `resolve`: `predicate/resolve/tests.rs`,
  `oracle_register.py` following it.
- **B30** `stream/tests.rs` tests six modules, the only reason
  `distribute`, `derived_source_span` and `activate` are wider than private.
- **B31** `DynamicRead`'s seven fields are `pub(in crate::stream)` for one
  struct literal in `partitions.rs`.
- **B32** `table_schema` repeats `ReplayPlan::new`'s opening, the only reason
  three plan items are wider than `replay/`.
- **B33** One field's text appended to a column is written three times
  (`render::decode_field`, `batch/builder.rs`'s `column_of`,
  `batch.rs`'s `push_field`).

datafusion-pgdump and datafusion-cli-pgdump
- **B34** The library's error becomes DataFusion's in four places that
  disagree: what an embedder downcasts to depends on the path, and a table
  refused by `PgDump::table` is wrapped twice. Not a cut alone: one
  conversion changes what a caller downcasts to.
- **B35** `table.rs`'s `scan` loosens the table's statistics under a pushed
  filter (D89, D91) inline; `statistics.rs`'s `byte_size` and
  `total_byte_size` are `pub(crate)` for it alone.
- **B36** `session_budget` installs the settings, and both registrations
  register the UDF beside it: one session install.
- **B37** The CLI recovers a table name and a reason by re-parsing the
  provider's `Located` format and the codec's note wording.
- **B38** The CLI installs the factory and then replaces it to hear when a
  registration ends.
- **B39** The mode words (`null|text|refuse`, `default|ignore`) are spelled
  in the factory, the CLI and `pgdt`'s clap enums; `StrictIdentity` already
  has `FromStr`/`Display`.
- **B40** A fixture mapped into a colocated cache, `parsed_copy`, exists in
  eight test variants: `pgdt-fixtures`, behind `mapping`.

pgdt
- **B41** `source.rs` holds `query`'s verbatim-name note and `info`'s
  unusable-cache sentence; its doc now says so.
- **B42** `args.rs` holds six flag items one subcommand reads; its doc now
  says so.
- **B43** `parse` reads `info::text`, `pub(crate)` for it, where `info` could
  re-export the listing.
- **B44** The root `tests.rs` reads `ParseArgs::identity`, the only reason it
  is `pub(crate)`.

**Kept:** `peak-rss` and `count-instructions` share a dozen lines of wrapper
protocol; they share no commits and each builds as its own static binary, so
the merge test is not met. The `module_graph.py --docs` links in every crate
name a caller or a producer, none a concept not standing alone; the reviews
list each.
