# P35.12 notes — the closing seams round

The round over the tree the cuts from 35.13 to 35.26 left. Its report named
no file past a hard limit and one past a soft one; this doc holds that file's
cut and what the read of every crate for the `seams` skill's "Antipatterns"
found, and the cuts the maintainer's rulings took. The cut and the read's
fixes changed no behaviour; the rulings on B13, B34, B36–B39 change public
API and what the front ends accept, as ruled ("As landed", below). Nothing
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

## The rulings

Each finding was re-read against the tree at `10107ad3` before it was ruled,
the reviews having been written before the round's own fixes; the maintainer
ruled on 2026-10-12. What the re-read corrected is stated with the ruling.

**Cut**, each in the commit that lands it:

- **B1** `instrument/` into three children, `statistics`, `evaluation`,
  `decode`, each a concern's API and hooks together; `internal` re-exports
  from them. The audience split never held: pgdt-query's evaluation reads
  `EvaluationPart`. Five `pub(super)` items, not six.
- **B2** `Cursor` into `type_name.rs`, renamed `identifier.rs`, its
  `internal` path kept. The wider scatter is concepts.md's "SQL lexing"
  **Not in one place** line: a qualified name read four ways, not two.
- **B3** the four items private; the docs linking `field_ranges` reworded.
- **B5** a `DecodedBlock` slice and a piece copy shared by `.xz`'s two arms,
  each keeping its own loop, which holds one cache slot at a time.
- **B6** the verdict once, behind `open`, the only production entry point;
  `open_local` and `open_remote` `pub(crate)`.
- **B7** the test into `partition/tests.rs`, the constant private.
- **B8** `ddl::classify`, `StatementShape` dissolved into it;
  `looks_like_framing_statement` stays the map's.
- **B9** `ddl/column.rs`, its tests with it, `measure.py`'s `PREAMBLE` too.
- **B10**, **B11** `ddl/lex.rs` renamed, and one keyword strip in
  `crate::lex`.
- **B12** `write_cache_file` in `file.rs`, called by `save` and by
  pgdt-query's forging test; no format change.
- **B14** `macaddr`'s octets and renderer into `network.rs`. The `timetz`
  half was misstated: `timetz_key` only composes its family's readers.
- **B16** `strtoint` private to `int.rs`, `HEX_NIBBLE` into `digits.rs`;
  `strtoint` also named a second, datetime function.
- **B17** the test into `declared/tests.rs`.
- **B18** `gathers_bounds` deleted; nothing has called it since P6.
- **B19** each half-split a method beside its type; `BoundsGatherer`'s
  append keeps the charge's ordering.
- **B20** one `bounded_columns` in `stored.rs`, KD115's entry saying its
  closure leaves it `pub(crate)`.
- **B21** one read-loop announcement in pgdt-dump's `scan.rs`, taking the
  parallelism; five sites in three crates read it.
- **B22** the account carried in `Pass::Parse`; pgdt-query's placeholder
  account and the `internal` line go.
- **B23**, its bounds half: `bounds/tests.rs`. Its density half waits on B4.
- **B24** the tree inverted: `interior.rs` the scheduler, a child `piece.rs`
  holding the piece scan, its fold and `BlockEndings`.
- **B25** `report_shortfall` beside `Shortfall`, one copy of the paragraph.
- **B26** `believed_under` beside `declared_column` in `stored.rs`. The
  fit guards differ legitimately: pruning addresses bytes.
- **B27** `conjuncts` beside `ResolvedExpr`, both flattenings built on it;
  `required_ordering_terms` stays in `truths.rs`, reading statistics.
- **B28** one `announced` derivation; the oracle test stays and proves the
  cut. Two derivations reading one register were never independent.
- **B30** the tests split across `replay/`, six items narrowed, not three.
- **B31** `DynamicRead::new`, the fields private.
- **B32** one opening shared by `table_schema` and `ReplayPlan::new`, so the
  provider's declared schema and the stream's agree by construction.
- **B33** a copying arm in `append_typed`.
- **B35** `scanned` and `fetched` both in `statistics.rs`; `exec.rs` held
  the fetch half of the same D89 clause.
- **B40** `parsed_copy` and `ungathered` into `pgdt-fixtures` behind
  `mapping`: thirteen copies and four, not eight.
- **B41** each helper into its one reader. The cache's two wordings are
  concepts.md's "The cache" **Not in one place** line.
- **B43** `info` re-exports what `parse` prints; `text` private.
- **B44** the test into `args/tests.rs`, `identity` private.

**Kept:** **B29** — `predicate/tests.rs` is the filter, resolved and
evaluated, against the server's answers, forcing nothing wider; its module
doc says so.

**Grilled**, the maintainer answering:

- **B4**, with **B23**'s density half: cut. `nth_smallest`, `min_rows_group`,
  `max_rows_group` and `BlockStatisticsExt` leave pgdt-model for a
  pgdt-mapping `statistics/density.rs`, beside `density_merges` and the
  density tests; they persist nothing and only pgdt-mapping reads them, so
  D68 already places them there and the move is a cut, not a crate decision.
- **B13**: cut. The two fields leave `ScanOptions` for a pgdt-mapping
  `GatherOptions`, taken by `map_file` and `gather_block_statistics` beside
  `ScanOptions`. Not `StatisticsRequest`: it is compared against a cache, and
  neither a run's allowance nor the mode may take part (D19; D103's rejected
  recording). D103's "(`ScanOptions`)" is reworded by the change that lands it.
- **B15**: cut. `grammar/form.rs` to `key/form.rs`, the clause struck from
  concepts.md's "Strict parse": the form is what `literal_key` accepts, as
  35.7 placed it, which 35.16's move of `key/order.rs`'s section broke.
- **B34**: cut, and no `KD<k>` unless it slips past the wrap. One rule:
  whatever crosses DataFusion's boundary is `External(datafusion_pgdump::Error)`,
  its `Library` variant no longer transparent but its own message over a
  `#[source]`, so a `source()` walk reaches `pgdt_core::Error` from every path;
  `PgDump::table` returns its own variants unwrapped. Today a scan gives the
  core error, `STORED AS PGDUMP` and `--dump` the provider's with the core
  error hidden from the walk, and `PgDump::table` its variants as
  `Error::DataFusion`; no doc promised a type, D74 only a downcast.
- **B36**: cut. One public `install(ctx)` — settings, budget, the
  `pgdump_unrepresentable` function — called by `register_dump` and the
  factory, and named by the docs for a `PgDumpTable` registered by hand, which
  today can call `unrepresentable::register` but never install the settings.
- **B37**, **B38**: cut. The library folds: each registration
  (`register_dump`, the factory's `create`) reports one structured finding of
  its uncollated columns — subject, columns, reason — in place of a note a
  column, so the CLI prints it, and its re-parse of the provider's
  `"{subject}: "` prefix and the note's wording, and the factory it installs
  only to replace, go. The prefix strip failed silently into one `""` table.
- **B39**: cut. Each mode enum states its words once, as `StrictIdentity`
  does, by `FromStr` and `Display`, **case-insensitive** as the factory
  already reads them; the factory and `--dump` parse through it, and `pgdt`
  reads the library types by a `value_parser`, its four clap enums gone.
  `PostgresInvalidValues` parses all three words, and the query front ends
  refuse `Strict`, a parse's alone (D103), through one library check. Today
  `--dump` and `pgdt`'s flags match case exactly while `PGDUMP` does not.
- **B42** follows from B39: four of its six items go with the clap enums, and
  `parse_statistics_level` and `parse_row_group_size` move into `parse.rs`,
  their one reader, as B41's helpers do.

The seam register is filed nothing: every finding is cut or kept.

### As landed

Where what landed differs from a ruling's words, or settles what it left
open:

- **B24**: `CensusPlan` and `Interior` are in `interior/piece.rs` too,
  re-exported by `interior.rs`: the ruled moves alone left `piece.rs` and
  its parent reading each other (`module_graph.py --design`).
- **B40**: `pgdt-fixtures`' `mapping` feature reaches pgdt-model, pgdt-io
  and pgdt-dump as well as pgdt-mapping, the crates `parsed_copy`'s call
  names; D68 says so.
- **B13**: `GatherOptions` is `pgdt_mapping::statistics::GatherOptions`, in
  `statistics/request.rs` beside `StatisticsRequest` and apart from it:
  defined beside `map_file` in `whole_file.rs` it made `whole_file` and
  `backfill` read each other. `map_forward` takes the mode in `Pass::Parse`.
- **B34**: `Error::Library`'s message is the core error's sentence, its
  source that error, so `datafusion-cli`, which prints the top error alone,
  prints what it did. `Error::DataFusion`, which only `PgDump::table`'s round
  trip produced, is gone; `From<Error> for DataFusionError` is the one
  conversion.
- **B36**: `install` returns the session's `ScanBudget`, which
  `register_dump` bills; `unrepresentable::register` and its
  `register_unrepresentable` re-export, needed by nothing alone, are folded
  into it.
- **B37**, **B38**: the finding is `datafusion_pgdump::UncollatedColumns`,
  its reason `ComparisonNote::bytewise_reason`, the clause every bytewise
  note's sentence ends with, stated once in pgdt-query.
  `PgDumpTable::report` folds too, being the factory's registration of one
  table.
- **B39**: each enum's words are its `Display`, `ALL` listing them;
  `FromStr` refuses another word by listing them. `pgdt`'s flags read the
  library types through `args.rs`'s `ModeParser`, a `TypedValueParser`
  whose `--help` values are the `Display` of the modes a flag accepts,
  which takes clap's `string` feature; every help page reads as before. The
  one check is `PostgresInvalidValues::queryable`. The factory's entry
  listing still shows a hand-set `Strict` as `default`, the mode a scan
  reads it as.

