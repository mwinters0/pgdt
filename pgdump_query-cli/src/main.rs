use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use clap::{Parser, Subcommand};
use pgdump_query::cache::CacheMode;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    BatchOptions, DataBlock, DeferredKind, DumpIndex, DumpMetadata, LocalFileSource, Predicate,
    PredicateOp, ScanOptions, Span, SpanBody, TypeKind, build_index, preamble_only, render_field,
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
/// `docs/design/roadmap-phase2-typed-columns.md`.
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
        file: PathBuf,
        /// Cache file path, or `none` to disable the cache. Since `parse`'s
        /// whole purpose is to write the cache, `none` is rejected.
        #[arg(long)]
        cache_path: Option<PathBuf>,
    },
    /// Print what is known about a dump file from its cache.
    Info {
        file: PathBuf,
        /// Cache file path, or `none` to ignore any existing cache and
        /// perform a fresh scan without persisting it.
        #[arg(long)]
        cache_path: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
        /// Answer from the preamble alone (dump-level header only, no
        /// per-block listing or row counts) instead of a full structural
        /// scan — cost is independent of dump size regardless of how much
        /// `COPY` data follows (`docs/design/roadmap-phase2-typed-columns.md`,
        /// "CLI").
        #[arg(long)]
        preamble_only: bool,
        /// List every span the full file map found (`docs/design/roadmap-phase3-object-inventory.md`,
        /// "The map is the structure, not a description of it") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing. Implies a full scan; incompatible with
        /// `--preamble-only`.
        #[arg(long)]
        map: bool,
    },
    /// Stream a table's rows, optionally filtered by a single-column predicate.
    Query {
        file: PathBuf,
        /// Table name, qualified (`schema.table`) or bare.
        table: String,
        /// Cache file path, or `none` to ignore any existing cache and
        /// perform a fresh scan without persisting it.
        #[arg(long)]
        cache_path: Option<PathBuf>,
        /// Single-column filter: `column=value`, `column!=value`,
        /// `column IS NULL`, or `column IS NOT NULL`, compared against each
        /// row's decoded field value.
        #[arg(long)]
        filter: Option<String>,
        /// Select which database to query when `table` is ambiguous across
        /// a multi-`\connect` dump (`docs/design/roadmap-phase2-typed-columns.md`,
        /// "One target per query").
        #[arg(long)]
        database: Option<String>,
        /// `typed` (default) resolves column types against the dump's DDL;
        /// `strings` skips that lookup entirely, matching Phase 1's
        /// byte-for-byte output — the way out of `Error::MetadataNotScanned`
        /// for a database an incremental scan hasn't read the DDL for yet.
        #[arg(long, value_enum, default_value_t)]
        schema_mode: CliSchemaMode,
    },
}

/// Parse a `--filter` argument into a [`Predicate`]: `column=value`,
/// `column!=value`, `column IS NULL`, or `column IS NOT NULL` (the `IS`
/// forms matched case-insensitively after the column name — see
/// `docs/design/roadmap-phase2-typed-columns.md`, "Predicates"). `!=` is
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
/// `typed` or `strings` (`docs/design/roadmap-phase2-typed-columns.md`,
/// "CLI").
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
        Command::Parse { file, cache_path } => {
            // `parse` is the eager entry point: always scan fresh (ignoring
            // any existing cache) and (re)write it, per the CLI spec in
            // `roadmap-phase1-mvp.md`.
            // Reject `--cache-path none` up front, before paying for a scan
            // we won't be allowed to persist.
            let mode = CacheMode::resolve(&file, cache_path.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--cache-path none` cannot be combined with `parse`")?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            let index = build_index(&source, &ScanOptions::default()).await?;
            print_index(&index, false, false);
            pgdump_query::cache::save(&path, &source, &index).await?;
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { file, cache_path, verbose, preamble_only: preamble_only_flag, map } => {
            if preamble_only_flag && map {
                anyhow::bail!("--preamble-only and --map cannot be combined");
            }
            let mode = CacheMode::resolve(&file, cache_path.as_deref());
            let source = LocalFileSource::open(&file)?;
            if preamble_only_flag {
                let metadata = preamble_only(&source, &ScanOptions::default(), &mode).await?;
                print_metadata(&metadata);
                return Ok(());
            }
            let index = match mode.load(&source).await? {
                Some(index) => index,
                None => {
                    // No usable (or disabled) cache: scan, then persist what
                    // we learned — a no-op under `CacheMode::Disabled` —
                    // `roadmap-phase1-mvp.md`'s "cache is never required for
                    // correctness" rule means this fallback must still
                    // produce a correct answer.
                    let index = build_index(&source, &ScanOptions::default()).await?;
                    mode.save(&source, &index).await?;
                    index
                }
            };
            print_index(&index, verbose, map);
        }
        Command::Query { file, table, cache_path, filter, database, schema_mode } => {
            let mode = CacheMode::resolve(&file, cache_path.as_deref());
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

/// Human-readable label for one column's resolution outcome — the `info
/// --verbose` per-column diagnostic line
/// (`docs/design/roadmap-phase2-typed-columns.md`, "CLI").
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
/// user-defined-type counts (`docs/design/roadmap-phase2-typed-columns.md`,
/// "CLI"). The `database: <name>` line is only shown when it's informative —
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
    // this listing already showed (`docs/design/roadmap-phase2-typed-columns.md`,
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

/// Referenced-role and referenced-tablespace summary
/// (`docs/design/roadmap-phase3-object-inventory.md`, "What a span carries")
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
/// (`docs/design/roadmap-phase3-object-inventory.md`, "TOC coverage is
/// recorded per file"). Counts `toc_owned` spans, not every attributed one:
/// this is an object *census*, and since slice 3.3.1 a follow-on statement
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
/// filters it down to (`docs/design/roadmap-phase3-object-inventory.md`,
/// "The map is the structure, not a description of it"). Same
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
