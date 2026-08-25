use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use clap::{Parser, Subcommand};
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    BatchOptions, ByteRangeSource, DataBlock, DeferredKind, Diagnostic, DiagnosticKind, DumpIndex,
    DumpMetadata, LocalFileSource, Predicate, PredicateOp, ScanOptions, Severity, Span, SpanBody,
    TypeKind, build_index, preamble_only, render_field,
};

#[derive(Parser)]
#[command(
    name = "pgdq",
    about = "Query pg_dump plain-format files without loading them into memory"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// CLI spelling of [`SchemaMode`] — see "Output model" in
/// `docs/design/architecture.md`.
#[derive(Clone, Copy, Default, clap::ValueEnum)]
enum CliSchemaMode {
    #[default]
    Typed,
    Strings,
}

impl From<CliSchemaMode> for SchemaMode {
    fn from(mode: CliSchemaMode) -> Self {
        match mode {
            CliSchemaMode::Typed => SchemaMode::Typed,
            CliSchemaMode::Strings => SchemaMode::Strings,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Perform a full file scan and build the structure cache.
    Parse {
        /// The dump file to scan.
        #[arg(long)]
        source: PathBuf,
        /// Cache file path, or `none` to disable the cache. Since `parse`'s
        /// whole purpose is to write the cache, `none` is rejected.
        #[arg(long)]
        dqcache: Option<PathBuf>,
    },
    /// Print what is known about a dump file — from a fresh or cached scan
    /// of the dump itself, or, with no `--source`, from a retained
    /// `--dqcache` alone once the dump is gone
    /// (`docs/design/architecture.md`, "The cache").
    Info {
        /// The dump file to scan. Omit it to answer from `--dqcache` alone
        /// — cache-only mode, which then requires `--dqcache`.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Cache file path. With `--source`: `none` ignores any existing
        /// cache and performs a fresh scan without persisting it, and
        /// omitting this flag resolves to the colocated default
        /// (`<source>.dqcache`). Without `--source`: required — this is the
        /// cache-only entry point, and there is nothing else to answer from.
        #[arg(long, required_unless_present = "source")]
        dqcache: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
        /// Answer from the preamble alone (dump-level header only, no
        /// per-block listing or row counts) instead of a full structural
        /// scan — cost is independent of dump size regardless of how much
        /// `COPY` data follows (`docs/design/architecture.md`,
        /// "CLI surface").
        #[arg(long)]
        preamble_only: bool,
        /// List every span the full file map found (`docs/design/architecture.md`,
        /// "`DumpIndex`: one owner per fact") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing. Implies a full scan; incompatible with
        /// `--preamble-only`.
        #[arg(long)]
        map: bool,
    },
    /// Stream a table's rows, optionally filtered by a single-column predicate.
    Query {
        /// The dump file to scan. `query` can never answer from a cache
        /// alone — row data is never cached — so this is always required.
        #[arg(long)]
        source: PathBuf,
        /// Table name, qualified (`schema.table`) or bare.
        #[arg(long)]
        table: String,
        /// Cache file path, or `none` to ignore any existing cache and
        /// perform a fresh scan without persisting it.
        #[arg(long)]
        dqcache: Option<PathBuf>,
        /// Single-column filter: `column=value`, `column!=value`,
        /// `column IS NULL`, or `column IS NOT NULL`, compared against each
        /// row's decoded field value.
        #[arg(long)]
        filter: Option<String>,
        /// Select which database to query when `table` is ambiguous across
        /// a multi-`\connect` dump (`docs/design/architecture.md`,
        /// "One target per query").
        #[arg(long)]
        database: Option<String>,
        /// `typed` (default) resolves column types against the dump's DDL;
        /// `strings` skips that lookup entirely, matching the untyped
        /// byte-for-byte output — the way out of `Error::MetadataNotScanned`
        /// for a database an incremental scan hasn't read the DDL for yet.
        #[arg(long, value_enum, default_value_t)]
        schema_mode: CliSchemaMode,
    },
}

/// Parse a `--filter` argument into a [`Predicate`]: `column=value`,
/// `column!=value`, `column IS NULL`, or `column IS NOT NULL` (the `IS`
/// forms matched case-insensitively after the column name — see
/// `docs/design/architecture.md`, "Predicates"). `!=` is
/// checked before `=` since it contains that byte.
fn parse_filter(spec: &str) -> Result<Predicate> {
    let trimmed = spec.trim_end();
    if let Some(column) = strip_ci_suffix(trimmed, "is not null") {
        return Ok(Predicate {
            column: column.trim_end().to_string(),
            op: PredicateOp::IsNotNull,
            value: None,
        });
    }
    if let Some(column) = strip_ci_suffix(trimmed, "is null") {
        return Ok(Predicate {
            column: column.trim_end().to_string(),
            op: PredicateOp::IsNull,
            value: None,
        });
    }
    let (column, op, value) = match spec.split_once("!=") {
        Some((column, value)) => (column, PredicateOp::Ne, value),
        None => match spec.split_once('=') {
            Some((column, value)) => (column, PredicateOp::Eq, value),
            None => anyhow::bail!(
                "--filter must be `column=value`, `column!=value`, `column IS NULL`, or `column IS NOT NULL`, got `{spec}`"
            ),
        },
    };
    Ok(Predicate { column: column.to_string(), op, value: Some(value.to_string()) })
}

/// Case-insensitive suffix strip, for matching `IS NULL`/`IS NOT NULL` at
/// the end of a `--filter` argument regardless of how the user cased it.
fn strip_ci_suffix<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let split = s.len().checked_sub(suffix.len())?;
    let (head, tail) = s.split_at(split);
    tail.eq_ignore_ascii_case(suffix).then_some(head)
}

/// Print one batch's rows tab-separated, `\N` for NULL — mirroring COPY
/// TEXT's own NULL marker. Each field is rendered back to PostgreSQL text via
/// [`render_field`], so output is byte-identical whether `--schema-mode` is
/// `typed` or `strings` (`docs/design/architecture.md`,
/// "CLI surface").
fn print_batch(batch: &RecordBatch) {
    for row in 0..batch.num_rows() {
        let fields: Vec<String> = batch
            .columns()
            .iter()
            .map(|c| render_field(c.as_ref(), row).unwrap_or_else(|| "\\N".to_string()))
            .collect();
        println!("{}", fields.join("\t"));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { source: file, dqcache } => {
            // `parse` is the eager entry point: always scan fresh (ignoring
            // any existing cache) and (re)write it, per the CLI spec in
            // `docs/design/architecture.md`.
            // Reject `--dqcache none` up front, before paying for a scan we
            // won't be allowed to persist.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--dqcache none` cannot be combined with `parse`")?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            let index = build_index(&source, &ScanOptions::default()).await?;
            print_index(&index, false, false);
            pgdump_query::cache::save(&path, &source, &index).await?;
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info {
            source: file,
            dqcache,
            verbose,
            preamble_only: preamble_only_flag,
            map,
        } => {
            if preamble_only_flag && map {
                anyhow::bail!("--preamble-only and --map cannot be combined");
            }
            let Some(file) = file else {
                // Cache-only mode (`docs/design/architecture.md`,
                // "The cache"): no live dump file at all, so
                // clap already required `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, verbose, preamble_only_flag, map).await;
            };
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let source = LocalFileSource::open(&file)?;
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(&source, &ScanOptions::default(), &mode).await?;
                print_metadata(&metadata);
                print_diagnostics(&diagnostics);
                return Ok(());
            }
            // A loaded cache is trusted for the full-file listing only once
            // it actually covers the whole file — a preamble-only or
            // still-incremental cache must trigger a fresh scan here rather
            // than being reported as if it were complete (the former
            // out-of-band item M1; `docs/design/architecture.md`,
            // "The cache").
            let file_size = source.size().await?;
            let index = match mode.load(&source).await? {
                Some(index) if index.scanned_through >= file_size => index,
                _ => {
                    // No usable cache, or one that doesn't cover the whole
                    // file: scan, then persist what we learned — a no-op
                    // under `CacheMode::Disabled` — the cache is never
                    // required for correctness (`docs/design/architecture.md`,
                    // "The cache"), which means
                    // this fallback must still produce a correct answer.
                    let index = build_index(&source, &ScanOptions::default()).await?;
                    mode.save(&source, &index).await?;
                    index
                }
            };
            print_index(&index, verbose, map);
        }
        Command::Query { source: file, table, dqcache, filter, database, schema_mode } => {
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let predicate = filter.as_deref().map(parse_filter).transpose()?;
            let source = LocalFileSource::open(&file)?;
            let mut header_printed = false;
            let mut rows = 0u64;
            let batch_options = BatchOptions {
                database,
                schema_mode: schema_mode.into(),
                ..BatchOptions::default()
            };
            let (_resolved_schema, _resume) = pgdump_query::read_table(
                &source,
                &table,
                &ScanOptions::default(),
                &batch_options,
                predicate,
                mode,
                |batch| {
                    if !header_printed {
                        let names: Vec<String> =
                            batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                        println!("{}", names.join("\t"));
                        header_printed = true;
                    }
                    print_batch(&batch);
                    rows += batch.num_rows() as u64;
                    ControlFlow::Continue(())
                },
            )
            .await?;
            if header_printed {
                eprintln!("{rows} row(s)");
            } else {
                eprintln!("no rows found for {table} in {}", file.display());
            }
        }
    }
    Ok(())
}

/// `pgdq info` with no `--source`: answer strictly from the cache at `path`,
/// erroring rather than falling back to a scan when it doesn't have enough
/// (`docs/design/architecture.md`, "The cache"). `--preamble-only`'s completeness bar is the metadata's own
/// `preamble_complete` flag rather than whole-file coverage, since a
/// preamble-only cache is exactly the shape that flag exists to recognize;
/// the default listing and `--map` both need the whole file mapped, so any
/// `Incomplete` cache is an error for them.
async fn info_offline(
    path: &Path,
    verbose: bool,
    preamble_only_flag: bool,
    map: bool,
) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let index = match mode.load_offline().await? {
        CacheStatus::Absent => {
            anyhow::bail!("no usable cache found at {}", path.display());
        }
        CacheStatus::Incomplete { index, total_size, .. } => {
            let preamble_complete = index
                .metadata
                .as_ref()
                .and_then(|m| m.databases.first())
                .is_some_and(|db| db.preamble_complete);
            if !(preamble_only_flag && preamble_complete) {
                anyhow::bail!(
                    "cache at {} only covers {} of {} bytes — cache-only mode cannot extend it; re-run with --source to finish the scan",
                    path.display(),
                    index.scanned_through,
                    total_size
                );
            }
            index
        }
        CacheStatus::Valid { index, .. } => index,
    };
    if preamble_only_flag {
        print_metadata(&index.metadata.clone().unwrap_or_default());
        print_diagnostics(&index.diagnostics);
    } else {
        print_index(&index, verbose, map);
    }
    Ok(())
}

/// Human-readable label for one column's resolution outcome — the `info
/// --verbose` per-column diagnostic line
/// (`docs/design/architecture.md`, "CLI surface").
fn resolution_label(r: &ColumnResolution) -> String {
    match r {
        ColumnResolution::Mapped => "mapped".to_string(),
        ColumnResolution::UnknownType => "unknown type — no mapping for this build".to_string(),
        ColumnResolution::NotDeclared => "not declared — no DDL explained this column".to_string(),
        ColumnResolution::Deferred { kind } => {
            let kind = match kind {
                DeferredKind::Array => "array",
                DeferredKind::Composite => "composite",
                DeferredKind::Range => "range",
            };
            format!("deferred ({kind}) — decodable, not yet implemented")
        }
        ColumnResolution::OpaqueBaseType => {
            "opaque base type — information-free in the dump".to_string()
        }
        ColumnResolution::EmptyEnum => "empty enum".to_string(),
    }
}

/// Dump-level metadata header: server/`pg_dump` versions, extension and
/// user-defined-type counts (`docs/design/architecture.md`,
/// "CLI surface"). The `database: <name>` line is only shown when it's informative —
/// a single unnamed database (a plain, non-`--create` dump: the overwhelming
/// common case) is printed with no header line, since one would just be
/// noise.
fn print_metadata(metadata: &DumpMetadata) {
    let multi = metadata.databases.len() > 1;
    for db in &metadata.databases {
        let show_name = multi || db.name.is_some();
        let indent = if show_name { "  " } else { "" };
        if show_name {
            match &db.name {
                Some(name) => println!("database: {name}"),
                None => println!("database: (unnamed)"),
            }
        }
        if let Some(v) = &db.server_version {
            println!("{indent}server version: {v}");
        }
        if let Some(v) = &db.pg_dump_version {
            println!("{indent}pg_dump version: {v}");
        }
        println!("{indent}extensions: {}", db.extensions.len());
        println!("{indent}user-defined types: {}", db.types.len());
    }
}

fn print_index(index: &DumpIndex, verbose: bool, map: bool) {
    if let Some(metadata) = &index.metadata {
        print_metadata(metadata);
        println!();
    }

    if print_diagnostics(&index.diagnostics) {
        println!();
    }

    let printed_roles = print_cross_references(index);
    let printed_kinds = print_object_kinds(index);
    if printed_roles || printed_kinds {
        println!();
    }

    if map {
        print_map(index);
        println!();
        println!("{} span(s), {} bytes scanned", index.spans.len(), index.scanned_through);
        return;
    }

    let blocks: Vec<_> = index.blocks().collect();
    if blocks.is_empty() {
        println!("no COPY blocks found in {} scanned bytes", index.scanned_through);
        return;
    }

    let mut total_columns = 0usize;
    let mut total_unmapped = 0usize;

    // Group by database once blocks carry more than one — the common case
    // (a plain or single-`--create` dump) prints no header at all. Blocks
    // are already in file order, and every `\connect` segment is contiguous
    // in the file, so a header line whenever the database changes is enough
    // — no need to sort or bucket first. This is also what makes an
    // `AmbiguousTable` error's candidate names actionable: they're names
    // this listing already showed (`docs/design/architecture.md`,
    // "One target per query").
    let multi_database =
        blocks.iter().map(|b| &b.database).collect::<std::collections::BTreeSet<_>>().len() > 1;
    let mut current_database: Option<&Option<String>> = None;

    for block in &blocks {
        if multi_database && current_database != Some(&block.database) {
            current_database = Some(&block.database);
            match &block.database {
                Some(name) => println!("database: {name}"),
                None => println!("database: (unnamed)"),
            }
        }
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        if block.header.columns.is_empty() {
            println!("    columns: (not listed in COPY header)");
        } else {
            let qualified = block.header.qualified_name();
            let resolved: ResolvedSchema = resolve_columns(
                &qualified,
                &block.header.columns,
                index.metadata.as_ref(),
                block.database.as_deref(),
                SchemaMode::Typed,
            );
            let columns: Vec<String> = resolved
                .notes
                .iter()
                .map(|d| match &d.declared {
                    Some(ty) => format!("{} {ty}", d.column),
                    None => format!("{} (unknown)", d.column),
                })
                .collect();
            println!("    columns: {}", columns.join(", "));
            if verbose {
                for note in
                    resolved.notes.iter().filter(|d| d.resolution != ColumnResolution::Mapped)
                {
                    println!("    {}: {}", note.column, resolution_label(&note.resolution));
                }
            }
            total_columns += resolved.notes.len();
            total_unmapped += resolved.unmapped_count();
        }
        if verbose {
            println!("    header offset: {}", block.header_offset);
            println!("    data offset:   {}", block.data_offset);
            println!("    terminator:    {}", block.terminator_offset);
            println!("    end offset:    {}", block.end_offset);
        }
    }

    println!();
    println!(
        "{} COPY block(s), {} row(s), {} bytes scanned",
        blocks.len(),
        index.total_rows(),
        index.scanned_through
    );
    if total_unmapped > 0 {
        println!(
            "{total_unmapped} of {total_columns} columns unmapped — run with --verbose for details"
        );
    }
}

/// `DumpIndex::diagnostics` (or, for `--preamble-only`, the diagnostics
/// `preamble_only` reports separately), printed unconditionally — this is
/// (`docs/design/architecture.md`, "The cache"). Cache-only mode's
/// "unverified, historical" banner rides this same path (`DiagnosticKind::CacheOffline`).
/// Returns whether anything was printed, matching `print_cross_references`'s
/// and `print_object_kinds`' convention.
fn print_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    if diagnostics.is_empty() {
        return false;
    }
    println!("diagnostics:");
    for d in diagnostics {
        println!("    [{}] {}", severity_label(d.severity), diagnostic_message(&d.kind));
    }
    true
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn diagnostic_message(kind: &DiagnosticKind) -> String {
    match kind {
        DiagnosticKind::TilingBroken { issues } => format!(
            "the file map has {} gap(s)/overlap(s) that don't tile the file — this is a pgdq bug, please report it",
            issues.len()
        ),
        DiagnosticKind::CacheMtimeChanged => "the dump file's mtime has changed since the cache was saved (size still matches, so the cache was kept)".to_string(),
        DiagnosticKind::TocCoverage { attributed, spans } => {
            format!("TOC coverage: {attributed}/{spans} span(s) attributed to a TOC entry")
        }
        DiagnosticKind::CacheOffline => {
            "answering from a cache with no source dump file to check it against — unverified, historical as of whenever the cache was last saved".to_string()
        }
    }
}

/// Referenced-role and referenced-tablespace summary
/// (`docs/design/architecture.md`, "TOC enrichment")
/// — an empty set prints nothing, so a dump referencing neither leaves no
/// trace here. Returns whether anything was printed, so the caller knows
/// whether to add a separating blank line.
fn print_cross_references(index: &DumpIndex) -> bool {
    let mut printed = false;
    if !index.roles.is_empty() {
        println!("roles: {}", index.roles.iter().cloned().collect::<Vec<_>>().join(", "));
        printed = true;
    }
    if !index.tablespaces.is_empty() {
        println!(
            "tablespaces: {}",
            index.tablespaces.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        printed = true;
    }
    printed
}

/// Per-`Type:` object-kind counts — one per archive entry, the same closed
/// ~63-value vocabulary the TOC-coverage diagnostic counts against
/// (`docs/design/architecture.md`, "TOC enrichment"). Counts `toc_owned`
/// spans, not every attributed one: this is an object *census*, and a
/// follow-on statement
/// (`ALTER ... OWNER TO`, etc.) inherits its governing entry's `toc` rather
/// than carrying `None` — counting `span.toc.is_some()` here would count that
/// object twice ("Span boundaries: statement-anchored, object-attributed,
/// greedy"). A span with no TOC comment at all (the header-less-input
/// fallback) contributes to no bucket here, since there is nothing typed to
/// count it under. Returns whether anything was printed.
fn print_object_kinds(index: &DumpIndex) -> bool {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for span in &index.spans {
        if span.toc_owned
            && let Some(toc) = &span.toc
        {
            *counts.entry(toc.kind.as_str()).or_default() += 1;
        }
    }
    if counts.is_empty() {
        return false;
    }
    println!("object kinds:");
    for (kind, count) in counts {
        println!("    {kind}: {count}");
    }
    true
}

/// `--map`: every span the full file map found, in file order — the raw
/// structure `DumpIndex::spans` keeps, not the per-table view `blocks()`
/// filters it down to (`docs/design/architecture.md`,
/// "`DumpIndex`: one owner per fact"). Same
/// database-header grouping convention as the ordinary block listing above.
fn print_map(index: &DumpIndex) {
    let multi_database =
        index.spans.iter().map(|s| &s.database).collect::<std::collections::BTreeSet<_>>().len()
            > 1;
    let mut current_database: Option<&Option<String>> = None;
    for span in &index.spans {
        if multi_database && current_database != Some(&span.database) {
            current_database = Some(&span.database);
            match &span.database {
                Some(name) => println!("database: {name}"),
                None => println!("database: (unnamed)"),
            }
        }
        println!("[{}, {}) {}", span.start, span.end, span_summary(span));
    }
}

/// One-line label for a span in `--map` output.
fn span_summary(span: &Span) -> String {
    match &span.body {
        SpanBody::Data(DataBlock::Copy(block)) => {
            format!("COPY {} ({} rows)", block.header.qualified_name(), block.row_count)
        }
        SpanBody::Data(DataBlock::InsertRun(run)) => {
            format!("INSERT run: {} ({} statements)", run.table, run.row_count)
        }
        SpanBody::Data(DataBlock::LargeObjects(_)) => "large objects".to_string(),
        SpanBody::Table { name, .. } => format!("TABLE {name}"),
        SpanBody::TypeDef { name, kind } => format!("TYPE {name} ({})", type_kind_label(kind)),
        SpanBody::Extension { name, .. } => format!("EXTENSION {name}"),
        SpanBody::Connect { database } => format!("\\connect {database}"),
        SpanBody::VersionHeader { .. } => "version header".to_string(),
        SpanBody::AlterTypeAddValue { type_name, label } => {
            format!("ALTER TYPE {type_name} ADD VALUE {label:?}")
        }
        SpanBody::Framing => "framing".to_string(),
        SpanBody::Unparsed => match &span.toc {
            Some(toc) => format!("{} {}", toc.kind, toc.name),
            None => "unparsed".to_string(),
        },
        SpanBody::Unscanned => "unscanned".to_string(),
    }
}

/// Short label for a [`TypeKind`] — `--map`'s compact form of the same
/// six-emission-shape vocabulary `docs/manual/type-handling.md` explains for
/// readers.
fn type_kind_label(kind: &TypeKind) -> &'static str {
    match kind {
        TypeKind::Enum { .. } => "enum",
        TypeKind::Domain { .. } => "domain",
        TypeKind::Composite { .. } => "composite",
        TypeKind::Range { .. } => "range",
        TypeKind::Base => "base",
        TypeKind::Shell => "shell",
    }
}
