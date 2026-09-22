# P6.8 — `datafusion-cli-pgdump`: notes

The last slice of [`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), "The
binary: `datafusion-cli-pgdump`". A new workspace member,
`datafusion-cli-pgdump`, is `datafusion-cli` 55.1.0 with `--dump` and
`STORED AS PGDUMP`, printing what each registration finds to stderr. The
user's page is [`../manual/datafusion-cli-pgdump.md`](../manual/datafusion-cli-pgdump.md).
The checks are `datafusion-pgdump/tests/registration.rs` for the provider's
half and `datafusion-cli-pgdump/tests/cli.rs` for the binary run as a user
runs it.

## What the wrap and later work inherit

- **The provider now takes a sink.** `register_dump` has a fourth argument,
  `&dyn DiagnosticSink`, and hands it the dump's file-level findings and then
  every table's column notes and `column_divergences` in Arrow semantics
  (`datafusion-pgdump/src/report.rs`). `PgDump::report` and
  `PgDumpTable::report(subject, sink)` are the same halves for a caller
  registering by hand.
- **A finding is named by a prefix, not a field.** The provider wraps each one
  so its `message` starts with the dump's origin or `catalog.schema.table`.
  `as_any` still returns the library's own record, so a sink can downcast to
  `Diagnostic`, `ColumnNote` or `ComparisonNote` as the library's rustdoc
  says. The subject is not recoverable as data. 6.3's notes flagged that a
  finding names no table.
- **A catalog registration builds every table's provider.** Reporting needs
  each table's resolved schema, so `PgDumpCatalog::report` builds and caches
  them all at registration rather than at the first `table()` call. A table
  whose plan refuses reports nothing then; its refusal surfaces when it is
  queried, as before.
- **`STORED AS PGDUMP` is the provider crate's, not the binary's.**
  `register_table_factory(ctx, sink)` registers the factory under
  `PGDUMP_FILE_TYPE`, the `PgDumpTableOptions` extension (prefix `pgdump`)
  and the session's budget. Any embedder gets the statement, not only the
  binary. The options are `pgdump.table` (required), `pgdump.schema`,
  `pgdump.database` and `pgdump.schema_mode`. Three keys rather than one
  dotted name, so a name holding `.` needs no quoting rule. The factory reads
  the keys through the extension's own `set`, so an unknown `pgdump.` key is
  refused even in a session that does not validate options, and so is a key
  in any other namespace. Declared columns, `PARTITIONED BY` and `WITH ORDER`
  are refused. It bills the dump's statistics at `create`, where 6.5.1's rule
  would have billed them at the first scan.
- **The binary is a marked copy.** `src/main.rs` is upstream's 55.1.0
  `main.rs` without its tests (they read upstream's test data), reformatted
  by this workspace's `rustfmt.toml`. Each of our additions carries a
  `pgdump:` comment, and everything else is in `src/pgdump.rs`. Re-copying at
  the next major means diffing upstream's new `main.rs` against 55.1.0's and
  re-applying the marked lines. The binary keeps upstream's `mimalloc`.
- **`--dump [NAME=]SOURCE[:strings]`.** `NAME=` is recognised only where the
  text before the first `=` holds no `/`, `\`, `.` or `:`, so a URL's query
  string is never a name (`DumpArg`'s rustdoc). Naming follows
  `register_dump`'s rule. A dump that cannot be opened or named ends the run
  before any SQL is read.
- **The stderr sink prints `warning:` and `error:` lines only.** It never
  prints an `Info`, and under `-q` it prints an `Error` alone. See "Decisions
  worth another look".
- **The ADBC floor is published and held to the pin.** The manual names
  `adbc-driver-postgresql` 1.12.0. `scripts/floor_mapping.py` now fails if
  that page does not name the release `scripts/pyproject.toml` pins, so
  moving the pin forces the page to change with the sweep.
- **The consistency scripts read the new crate.** `citations.py`'s
  `RUST_ROOTS`, `repoint.py`'s and `deficiencies.py`'s `CODE_ROOTS` gained
  it, as their own tests require of every workspace member. The copied
  upstream comments count toward `repoint.py`'s meter.

## Negative results

- **Nothing a query raises reaches the sink.** The spec prints "after the
  statement for what a query raised". In Arrow semantics the query-time
  comparison channel is empty: every operator the library answers, it
  answers as DataFusion does and announces nothing (`predicate.rs`, the walk
  over `ARROW_AGREEMENT`). The other two per-query channels,
  `TableStream::plan_notes` and `early_stops`, do not implement `Finding`
  (6.3's notes). So the binary installs no after-statement hook. Such a hook
  would mean owning the REPL loop, which the spec refuses. 6.8.1 reports a
  plan's notes at planning instead.
- **Registration on a real dump is loud.** Every `text` column with no
  `COLLATE` clause warns that its comparison is bytewise, because a plain dump
  does not record the database's collation. So attaching a dump prints one
  line per such column in every table. `M132` folds them into one line per
  dump in the binary, and a `--create` dump's stated collation going unread
  is `KD40`.
- **`datafusion-cli` reads `LOCATION` before the factory does.** Its
  `create_plan` parses the location as a `ListingTableUrl` and registers an
  object store for its scheme. For a path or an `http(s)` URL this is
  harmless: the store is registered and never read, because the provider
  reads through `Origin::resolve`.
- **`cargo build -p pgdt` is unaffected by the new dependency tree.** Cargo
  unifies features only across the packages it builds. No version already in
  `Cargo.lock` moved.
- **No `D<k>` entry.** Each shape is stated in the rustdoc beside it, and the
  spec holds the decisions. No figure was taken and no cache format moved.
