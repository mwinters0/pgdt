use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::pgtype::RANGE_STRUCT_FIELDS;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    BatchOptions, ByteRangeSource, DataBlock, Diagnostic, DiagnosticKind, DumpIndex, DumpMetadata,
    LocalFileSource, NestedPlan, Predicate, PredicateOp, ScanOptions, Severity, Span, SpanBody,
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
        /// Print the internal index as JSON instead of the human-readable
        /// listing: the whole `DumpIndex` plus diagnostics, or, with
        /// `--preamble-only`, just the metadata plus diagnostics. No schema
        /// stability is promised — this is a raw dump of our internal
        /// representation, not a supported interchange format
        /// (`docs/design/architecture.md`, "CLI surface"). Incompatible with
        /// `--verbose`/`--map`, which format detail this already carries in
        /// full.
        #[arg(long)]
        json: bool,
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
///
/// `plans` is the stream's own [`pgdump_query::ResolvedSchema::plans`], which
/// is what says whether a `List<Struct{…}>` column is written as an array of
/// ranges or as a multirange. A column with no entry falls back to
/// `NestedPlan::Scalar`, which is right for every non-nested type.
fn print_batch(batch: &RecordBatch, plans: &[NestedPlan]) {
    for row in 0..batch.num_rows() {
        let fields: Vec<String> = batch
            .columns()
            .iter()
            .enumerate()
            .map(|(col, c)| {
                let plan = plans.get(col).unwrap_or(&NestedPlan::Scalar);
                render_field(c.as_ref(), row, plan).unwrap_or_else(|| "\\N".to_string())
            })
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
            json,
        } => {
            if preamble_only_flag && map {
                anyhow::bail!("--preamble-only and --map cannot be combined");
            }
            if json && (verbose || map) {
                anyhow::bail!(
                    "--json already carries everything --verbose/--map would add — combine it with --preamble-only instead, or drop --json"
                );
            }
            let Some(file) = file else {
                // Cache-only mode (`docs/design/architecture.md`,
                // "The cache"): no live dump file at all, so
                // clap already required `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, verbose, preamble_only_flag, map, json).await;
            };
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let source = LocalFileSource::open(&file)?;
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(&source, &ScanOptions::default(), &mode).await?;
                if json {
                    print_metadata_json(&metadata, &diagnostics);
                } else {
                    print_metadata(&metadata);
                    print_diagnostics(&diagnostics);
                }
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
            if json {
                print_index_json(&index);
            } else {
                print_index(&index, verbose, map);
            }
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
            // Pull mode, not `read_table`: rendering a nested column back to
            // its literal needs the stream's `NestedPlan`s, and push mode
            // only hands the resolved schema back once the whole stream has
            // been drained (`docs/design/architecture.md`, "Arrow assembly
            // and the zero-copy path"). The scan itself is the same one —
            // `read_table` drains this stream internally.
            let mut stream = pgdump_query::table_stream(
                &source,
                &table,
                ScanOptions::default(),
                batch_options,
                predicate,
                None,
                mode,
            );
            while let Some(batch) = stream.next().await.transpose()? {
                if !header_printed {
                    let names: Vec<String> =
                        batch.schema().fields().iter().map(|f| f.name().clone()).collect();
                    println!("{}", names.join("\t"));
                    header_printed = true;
                }
                // Re-read per batch: a table's blocks each carry their own
                // schema (a header-less block names its columns from its
                // first row), so the plans belong to the block the batch came
                // from, not to the query.
                print_batch(&batch, &stream.resolved_schema().plans);
                rows += batch.num_rows() as u64;
            }
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
    json: bool,
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
        let metadata = index.metadata.clone().unwrap_or_default();
        if json {
            print_metadata_json(&metadata, &index.diagnostics);
        } else {
            print_metadata(&metadata);
            print_diagnostics(&index.diagnostics);
        }
    } else if json {
        print_index_json(&index);
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
        ColumnResolution::OpaqueElementType => {
            "opaque element type — the array's element type is information-free in the dump"
                .to_string()
        }
        ColumnResolution::OpaqueBaseType => {
            "opaque base type — information-free in the dump".to_string()
        }
        ColumnResolution::EmptyEnum => "empty enum".to_string(),
    }
}

/// What one column became in Arrow — the other half of `info --verbose`'s
/// per-column line (`docs/design/architecture.md`, "CLI surface").
///
/// Arrow's own `Display` is terse and reversible (`List(Utf8View)`,
/// `Struct("x": Int32, "y": Utf8View)`), and a composite's field names are the
/// user's own, so it carries real information and is what prints — with one
/// substitution. The five-field range struct is identical for every range
/// column in every dump and renders as 137 characters saying so, so it
/// collapses to `Range<T>`, `T` being the bound type: the only part that
/// varies. The manual states the struct's real layout once, which is what
/// makes the elision lossless.
///
/// **The substitution is detected from the [`NestedPlan`], never from the
/// field names** — a user composite is free to declare five fields with
/// exactly those names, and `pgtype::RANGE_STRUCT_FIELDS` reserves dispatch to
/// the plan. A built-in multirange and an array of the matching range render
/// *identically* (`List(Range<Int32>)`), which is correct rather than a
/// collision to fix: they are the same Arrow type, the plans differ, and the
/// declared PostgreSQL type sits on the same line.
///
/// The type and the plan come from one producer and cannot disagree; this
/// being display code, a disagreeing pair falls back to plain `Display`
/// rather than panicking the way the builder does.
fn arrow_type_label(data_type: &DataType, plan: &NestedPlan) -> String {
    match (plan, data_type) {
        (NestedPlan::Array(element), DataType::List(field)) => {
            format!("List({})", arrow_type_label(field.data_type(), element))
        }
        (NestedPlan::Record(field_plans), DataType::Struct(fields))
            if field_plans.len() == fields.len() =>
        {
            let rendered: Vec<String> = fields
                .iter()
                .zip(field_plans)
                .map(|(f, p)| format!("{:?}: {}", f.name(), arrow_type_label(f.data_type(), p)))
                .collect();
            format!("Struct({})", rendered.join(", "))
        }
        (NestedPlan::Range(bound), _) => range_label(data_type, bound),
        (NestedPlan::Multirange(bound), DataType::List(field)) => {
            format!("List({})", range_label(field.data_type(), bound))
        }
        _ => data_type.to_string(),
    }
}

/// The `Range<T>` substitution itself, shared by the `Range` and `Multirange`
/// plans — the latter is a `List` of exactly this struct.
fn range_label(data_type: &DataType, bound: &NestedPlan) -> String {
    match data_type {
        DataType::Struct(fields) if fields.len() == RANGE_STRUCT_FIELDS.len() => {
            format!("Range<{}>", arrow_type_label(fields[0].data_type(), bound))
        }
        _ => data_type.to_string(),
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

/// `--json`'s full-scan shape: the whole [`DumpIndex`] flattened to one
/// object, with diagnostics added back in — `DumpIndex::diagnostics` is
/// `#[serde(skip)]` for the cache's own reasons (`docs/design/architecture.md`,
/// "The cache"), which don't apply here, so this wrapper is the convenient
/// way to get them back without touching that skip. No schema stability is
/// promised for any of this — see the `--json` flag's help text.
#[derive(serde::Serialize)]
struct IndexJson<'a> {
    #[serde(flatten)]
    index: &'a DumpIndex,
    diagnostics: &'a [Diagnostic],
}

/// `--json --preamble-only`'s shape: just the metadata plus diagnostics,
/// matching what the human-readable preamble-only path prints.
#[derive(serde::Serialize)]
struct MetadataJson<'a> {
    #[serde(flatten)]
    metadata: &'a DumpMetadata,
    diagnostics: &'a [Diagnostic],
}

fn print_index_json(index: &DumpIndex) {
    let wrapped = IndexJson { index, diagnostics: &index.diagnostics };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("DumpIndex is always valid JSON"));
}

fn print_metadata_json(metadata: &DumpMetadata, diagnostics: &[Diagnostic]) {
    let wrapped = MetadataJson { metadata, diagnostics };
    println!(
        "{}",
        serde_json::to_string_pretty(&wrapped).expect("DumpMetadata is always valid JSON")
    );
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
                // One line per column that has something to say. A column
                // that did not map says why; a column that mapped says what
                // it mapped *to*, unless that is `Utf8View` — the
                // no-information answer, and the only Arrow type a
                // non-`Mapped` resolution ever produces, so the two arms
                // never both fire.
                for (i, note) in resolved.notes.iter().enumerate() {
                    let data_type = resolved.schema.field(i).data_type();
                    if note.resolution != ColumnResolution::Mapped {
                        println!("    {}: {}", note.column, resolution_label(&note.resolution));
                    } else if *data_type != DataType::Utf8View {
                        println!(
                            "    {}: {}",
                            note.column,
                            arrow_type_label(data_type, &resolved.plans[i])
                        );
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{Field, Fields};
    use std::sync::Arc;

    fn list_of(child: DataType) -> DataType {
        DataType::List(Arc::new(Field::new("item", child, true)))
    }

    fn range_struct(bound: DataType) -> DataType {
        DataType::Struct(Fields::from(vec![
            Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
            Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
            Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
            Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
        ]))
    }

    /// A scalar column's Arrow type is arrow's own `Display`, unmodified —
    /// which is the half of the line that was never visible before, and the
    /// reason the line is printed for every mapped non-`Utf8View` column
    /// rather than only for nested ones.
    #[test]
    fn a_scalar_column_renders_as_arrows_own_display() {
        assert_eq!(
            arrow_type_label(&DataType::Decimal128(38, 10), &NestedPlan::Scalar),
            "Decimal128(38, 10)"
        );
    }

    #[test]
    fn arrays_and_composites_render_through_arrows_display() {
        assert_eq!(
            arrow_type_label(
                &list_of(DataType::Utf8View),
                &NestedPlan::Array(Box::new(NestedPlan::Scalar))
            ),
            "List(Utf8View)"
        );
        let point = DataType::Struct(Fields::from(vec![
            Field::new("x", DataType::Int32, true),
            Field::new("y", DataType::Utf8View, true),
        ]));
        assert_eq!(
            arrow_type_label(&point, &NestedPlan::Record(vec![NestedPlan::Scalar; 2])),
            r#"Struct("x": Int32, "y": Utf8View)"#
        );
    }

    /// The one substitution: the five-field range struct is identical in
    /// every dump, so only its bound type is worth printing.
    #[test]
    fn the_range_struct_collapses_to_its_bound_type() {
        assert_eq!(
            arrow_type_label(
                &range_struct(DataType::Int32),
                &NestedPlan::Range(Box::new(NestedPlan::Scalar))
            ),
            "Range<Int32>"
        );
    }

    /// A built-in multirange and an array of the matching range are the
    /// *same* Arrow type and different plans, so rendering identically is
    /// correct rather than a collision — the declared PostgreSQL type sits on
    /// the same line and tells them apart.
    #[test]
    fn a_multirange_and_an_array_of_the_matching_range_render_identically() {
        let multirange = arrow_type_label(
            &list_of(range_struct(DataType::Int32)),
            &NestedPlan::Multirange(Box::new(NestedPlan::Scalar)),
        );
        let array_of_range = arrow_type_label(
            &list_of(range_struct(DataType::Int32)),
            &NestedPlan::Array(Box::new(NestedPlan::Range(Box::new(NestedPlan::Scalar)))),
        );
        assert_eq!(multirange, "List(Range<Int32>)");
        assert_eq!(array_of_range, multirange);
    }

    /// Dispatch is the plan's, never the field names' — a user composite may
    /// declare five fields with exactly the range struct's names, and it must
    /// still print as the struct it is.
    #[test]
    fn a_composite_wearing_the_range_structs_field_names_is_not_collapsed() {
        let impostor = range_struct(DataType::Int32);
        let rendered =
            arrow_type_label(&impostor, &NestedPlan::Record(vec![NestedPlan::Scalar; 5]));
        assert!(rendered.starts_with(r#"Struct("lower": Int32"#), "{rendered}");
        assert!(!rendered.contains("Range<"), "{rendered}");
    }
}
