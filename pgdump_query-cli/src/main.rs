use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::DataType;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use pgdump_query::cache::{CacheMode, CacheStatus};
use pgdump_query::pgtype::RANGE_STRUCT_FIELDS;
use pgdump_query::resolve::{ColumnResolution, ResolvedSchema, SchemaMode, resolve_columns};
use pgdump_query::{
    ArrayShape, ByteRangeSource, DataBlock, Diagnostic, DiagnosticKind, DumpIndex, DumpMetadata,
    LocalFileSource, NestedPlan, Predicate, PredicateOp, QueryOptions, ScanOptions, Severity, Span,
    SpanBody, TypeKind, preamble_only, render_field,
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
    /// Scan a dump file and build the structure cache — the only command
    /// that reads the dump for its structure (`pgdq info` reports from the
    /// cache this leaves). Resumes from a matching cache rather than
    /// restarting, and banks its progress at `COPY` block boundaries as it
    /// goes — including on Ctrl-C, which saves what has been scanned and
    /// exits 130 — so an interrupted scan is not wasted work. Remove the
    /// cache file to force a scan from byte 0.
    Parse {
        /// The dump file to scan.
        #[arg(long)]
        source: PathBuf,
        /// Cache file path, or `none` to disable the cache. Since `parse`'s
        /// whole purpose is to write the cache, `none` is rejected.
        #[arg(long)]
        dqcache: Option<PathBuf>,
        /// Scan only the dump's preamble — the dump-level header alone, no
        /// per-block listing or row counts — instead of the whole file. Cost
        /// is independent of dump size regardless of how much `COPY` data
        /// follows (`docs/design/architecture.md`, "Bounded preamble-only
        /// reads"). The cache this leaves is a partial one that `pgdq info`
        /// reads like any other.
        #[arg(long)]
        preamble_only: bool,
    },
    /// Report what a dump's cache holds. **`info` never scans** — it reads the
    /// cache `pgdq parse` wrote and errors if there is not one, rather than
    /// starting an hours-long scan on your behalf
    /// (`docs/design/architecture.md`, "CLI surface"). A cache from an
    /// unfinished scan is reported for as far as it got, with the coverage
    /// stated at the top.
    Info {
        /// The dump file the cache belongs to: its size is checked against
        /// the cache's, so a changed file is caught. Omit it to answer from
        /// `--dqcache` alone — cache-only mode, which then requires
        /// `--dqcache` and cannot check anything.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Cache file path, defaulting to the colocated `<source>.dqcache`.
        /// Required when `--source` is omitted — that is the cache-only entry
        /// point, and there is nothing else to answer from.
        #[arg(long, required_unless_present = "source")]
        dqcache: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
        /// List every span the map holds (`docs/design/architecture.md`,
        /// "`DumpIndex`: one owner per fact") — DDL objects
        /// and framing included, not just `COPY` blocks — instead of the
        /// per-table listing.
        #[arg(long)]
        map: bool,
        /// Print the internal index as JSON instead of the human-readable
        /// listing: the whole `DumpIndex`, its coverage, its diagnostics, and
        /// the per-`COPY`-block type resolution `--verbose` renders as text.
        /// No schema stability is promised — this is a raw dump of our
        /// internal representation, not a supported interchange format
        /// (`docs/design/architecture.md`, "CLI surface"). Incompatible with
        /// `--verbose`/`--map`, which format detail this already carries in
        /// full.
        #[arg(long)]
        json: bool,
    },
    /// Stream a table's rows, optionally projected to named columns and
    /// filtered by a single-column predicate.
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
        /// Materialize only this column, repeatable — the output carries the
        /// columns in the order the flags give them, which need not be the
        /// file's. A name the table does not carry is an error, and so is a
        /// repeated one. Omit it entirely for every column
        /// (`docs/design/architecture.md`, "Projection").
        ///
        /// A column that is not projected is never decoded, so projecting a
        /// column away is also the way past a value that fails to decode
        /// while keeping every other column typed.
        ///
        /// A `--filter` may name a column this does not: the projection
        /// decides what is built, never what may be tested.
        #[arg(long = "column", value_name = "NAME")]
        column: Vec<String>,
        /// Materialize no columns at all — the `COUNT(*)` shape. Each row
        /// prints as an empty line and no header line is printed, so
        /// `--no-columns | wc -l` is a row count. Incompatible with
        /// `--column`.
        #[arg(long, conflicts_with = "column")]
        no_columns: bool,
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

/// Turn the two projection flags into [`QueryOptions::projection`]. The three
/// states are distinct and none of them is spelled the same way:
/// `--no-columns` is the empty projection (`COUNT(*)`), one or more
/// `--column` is that list in that order, and neither flag is `None` — every
/// column (`docs/design/architecture.md`, "Projection").
///
/// The two flags cannot both be given: clap's `conflicts_with` refuses that
/// before this is reached, so `--no-columns` wins here only in a case that
/// cannot occur. Nothing rejects a repeated `--column` name at this layer —
/// the library refuses it as `Error::DuplicateProjectionColumn` before a byte
/// of the file is read, which is the same answer with the same wording
/// whether the caller is the CLI or an embedder.
fn projection(columns: Vec<String>, no_columns: bool) -> Option<Vec<String>> {
    if no_columns {
        Some(Vec::new())
    } else if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
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

/// The one line `pgdq parse` prints about *this invocation* rather than about
/// the file: where the scan picked up. `None` for a scan that started at byte
/// 0, which is the case that needs no explanation.
///
/// A run that found the cache already complete scanned nothing at all, and
/// says so rather than reporting a resume point equal to the file's size —
/// the two are different facts to a user checking whether an interrupted scan
/// finished.
fn resume_notice(resumed_from: u64, size: u64) -> Option<String> {
    match resumed_from {
        0 => None,
        n if n >= size => {
            Some(format!("nothing to scan: the cache already covers all {size} byte(s)"))
        }
        n => Some(format!("resumed a previous scan at byte {n} of {size}")),
    }
}

/// Catch `SIGINT` and `SIGTERM` for the duration of a scan, so an interrupted
/// `pgdq parse` saves what it has instead of throwing it away
/// (`docs/design/architecture.md`, "`parse` resumes, and saves as it goes").
///
/// The guard is **cooperative**: the signal sets a flag the mapping loop reads
/// once per chunk, and the loop persists the index it owns before returning.
/// *Rejected:* `tokio::select!` in the CLI over `ctrl_c` and the scan future.
/// It reads as the obvious form and it is the one that silently discards the
/// work — `map_file` owns the `DumpIndex` for the whole scan, so cancelling
/// that future drops the map rather than saving it.
///
/// A **second** signal, of either kind, exits immediately: a save that wedges
/// must not be able to hold the process, and a Ctrl-C that appears to do
/// nothing is worse than no handler at all.
///
/// Returns the cell the exit code is read from: `0` until a signal lands,
/// then that signal's number.
fn install_interrupt_guard(cancel: Arc<AtomicBool>) -> Result<Arc<AtomicI32>> {
    use tokio::signal::unix::{SignalKind, signal};

    let signalled = Arc::new(AtomicI32::new(0));
    for kind in [SignalKind::interrupt(), SignalKind::terminate()] {
        let number = kind.as_raw_value();
        let mut stream =
            signal(kind).with_context(|| format!("installing handler for {number}"))?;
        let (cancel, signalled) = (Arc::clone(&cancel), Arc::clone(&signalled));
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                // A non-zero previous value means the other handler, or this
                // one, has already asked the scan to stop.
                let already = signalled.swap(number, Ordering::SeqCst);
                cancel.store(true, Ordering::SeqCst);
                if already != 0 {
                    std::process::exit(128 + number);
                }
            }
        });
    }
    Ok(signalled)
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
        Command::Parse { source: file, dqcache, preamble_only: preamble_only_flag } => {
            // `parse` is the only scanner (`docs/design/architecture.md`,
            // "CLI surface"). Reject `--dqcache none` up front, before paying
            // for a scan we won't be allowed to persist.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("parse")
                .context("`--dqcache none` cannot be combined with `parse`")?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            if preamble_only_flag {
                let (metadata, diagnostics) =
                    preamble_only(&source, &ScanOptions::default(), &mode).await?;
                print_metadata(&metadata);
                print_diagnostics(&diagnostics);
                println!();
                println!("wrote cache to {}", path.display());
                return Ok(());
            }
            let size = source.size().await?;
            let cancel = Arc::new(AtomicBool::new(false));
            let signalled = install_interrupt_guard(Arc::clone(&cancel))?;
            let scan_options = ScanOptions { cancel: Some(cancel), ..ScanOptions::default() };
            let run = pgdump_query::map_file(&source, &scan_options, &mode).await?;
            if run.interrupted {
                // No listing: the user asked the scan to stop, not for a
                // report on what it had reached, and `pgdq info` is the
                // command that reports. Both lines go to stderr, so a caller
                // redirecting stdout gets an empty report rather than a
                // truncated one.
                eprintln!(
                    "interrupted at byte {} of {size} — the cache at {} holds the scan so far",
                    run.index.scanned_through,
                    path.display()
                );
                eprintln!("re-run `pgdq parse --source {}` to continue", file.display());
                // Exit by signal (130/143), so a script can tell an interrupt
                // from a failure. `SIGINT` is the fallback for a flag nothing
                // in this binary sets any other way.
                let number = signalled.load(Ordering::SeqCst);
                std::process::exit(128 + if number == 0 { 2 } else { number });
            }
            // The listing describes the file's state after this run, not this
            // invocation's diff — so the one line that *is* about the
            // invocation goes above it, where a user checking on an
            // interrupted scan looks first.
            if let Some(notice) = resume_notice(run.resumed_from, size) {
                println!("{notice}");
                println!();
            }
            // `map_file` reached EOF, so its censuses cover the whole file.
            print_index(&run.index, false, false, true);
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { source: file, dqcache, verbose, map, json } => {
            if json && (verbose || map) {
                anyhow::bail!(
                    "--json already carries everything --verbose/--map would add — drop one of them"
                );
            }
            let Some(file) = file else {
                // Cache-only mode (`docs/design/architecture.md`,
                // "The cache"): no live dump file at all, so
                // clap already required `--dqcache` for us.
                let path = dqcache.expect("clap requires --dqcache when --source is omitted");
                return info_offline(&path, verbose, map, json).await;
            };
            // `info` never scans, so `--dqcache none` — "ignore the cache" —
            // would leave nothing at all to answer from. The message names the
            // way out, the way `Error::FieldDecode` names `--schema-mode
            // strings`: someone reaching for `none` is usually reaching for it
            // because the dump's own directory is read-only, and what they
            // want is a cache written somewhere else.
            //
            // Formed here rather than in `Error::CacheDisabled` because it
            // interpolates the user's own `--source` path, which the library
            // error does not have and should not take a `PathBuf` to get.
            // `FieldDecode` names a *static* flag string, which is why that
            // one could live in the error.
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let path = mode
                .require_enabled("info")
                .with_context(|| {
                    format!(
                        "`--dqcache none` cannot be combined with `info`, which never scans — run \
                         `pgdq parse --source {} --dqcache <path>` to build a cache somewhere \
                         writable, then pass that same `--dqcache <path>` here",
                        file.display()
                    )
                })?
                .to_path_buf();
            let source = LocalFileSource::open(&file)?;
            let status = pgdump_query::cache::load(&path, &source).await?;
            let (mut index, mtime_changed, total_size) = match status {
                CacheStatus::Valid { index, mtime_changed, total_size }
                | CacheStatus::Incomplete { index, mtime_changed, total_size } => {
                    (index, mtime_changed, total_size)
                }
                unusable => anyhow::bail!(unusable_cache_message(&unusable, &path, Some(&file))),
            };
            if mtime_changed {
                index.diagnostics.push(Diagnostic::cache_mtime_changed());
            }
            report(&index, total_size, verbose, map, json);
        }
        Command::Query {
            source: file,
            table,
            dqcache,
            filter,
            column,
            no_columns,
            database,
            schema_mode,
        } => {
            let mode = CacheMode::resolve(&file, dqcache.as_deref());
            let predicate = filter.as_deref().map(parse_filter).transpose()?;
            let source = LocalFileSource::open(&file)?;
            let mut header_printed = false;
            let mut any_batch = false;
            let mut rows = 0u64;
            let query_options = QueryOptions {
                database,
                schema_mode: schema_mode.into(),
                filter: predicate,
                projection: projection(column, no_columns),
                ..QueryOptions::default()
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
                query_options,
                None,
                mode,
            );
            while let Some(batch) = stream.next().await.transpose()? {
                any_batch = true;
                // A zero-column projection prints no header. The header would
                // be an empty line, and the row count `--no-columns | wc -l`
                // is asked for would come back one too many
                // (`docs/design/architecture.md`, "Projection").
                if !header_printed && batch.num_columns() > 0 {
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
            if any_batch {
                eprintln!("{rows} row(s)");
            } else {
                eprintln!("no rows found for {table} in {}", file.display());
            }
        }
    }
    Ok(())
}

/// The sentence `pgdq info` prints for a cache it cannot use. All four causes
/// end in `pgdq parse`, and they are still four different sentences: the
/// remedy is the same, the fact the user needs to know is not — "you have
/// never parsed this file" and "your file changed since you parsed it" send a
/// reader to different places.
///
/// **One match, two renderings**, the same discipline [`resolution_words`]
/// applies. `source` is `None` in cache-only mode, which has no dump file to
/// name and so states the fault and stops; the two paths otherwise describe
/// the same faults, and a second match is how they come to describe them
/// differently. Cache-only mode cannot reach
/// [`CacheStatus::SourceChanged`] at all — there is no live file to compare
/// against, which is exactly what its `CacheOffline` diagnostic warns about.
///
/// Takes the whole [`CacheStatus`] rather than a narrowed type so the match
/// stays exhaustive: a usable status reaching here is a caller bug, and it says
/// so rather than printing a plausible error.
fn unusable_cache_message(status: &CacheStatus, path: &Path, source: Option<&Path>) -> String {
    // Each arm supplies its own connective and tail, because "no cache at X"
    // and "X is not a pgdq cache" do not join to the same sentence.
    let remedy = |lead: &str, tail: &str| match source {
        Some(s) => format!(" — {lead}run `pgdq parse --source {}`{tail}", s.display()),
        None => String::new(),
    };
    match status {
        CacheStatus::Missing => {
            format!("no cache at {}{}", path.display(), remedy("", " first"))
        }
        CacheStatus::Unreadable => {
            format!("{} is not a pgdq cache{}", path.display(), remedy("check the path, or ", ""))
        }
        CacheStatus::UnsupportedVersion => format!(
            "the cache at {} was written by a different pgdq build and cannot be read{}",
            path.display(),
            remedy("", "")
        ),
        CacheStatus::SourceChanged { cached_size, live_size } => {
            let source = source.expect("cache-only mode has no live source to compare against");
            format!(
                "{} has changed since it was parsed ({live_size} bytes now, {cached_size} when \
                 the cache at {} was written), so every offset in the cache could be wrong{}",
                source.display(),
                path.display(),
                remedy("", "")
            )
        }
        CacheStatus::Valid { .. } | CacheStatus::Incomplete { .. } => {
            unreachable!("a usable cache is reported, not refused")
        }
    }
}

/// `pgdq info` with no `--source`: answer strictly from the cache at `path`
/// (`docs/design/architecture.md`, "The cache"). An `Incomplete` cache is
/// reported like any other, with its coverage stated — cache-only mode has no
/// scan to extend it with, but "as far as the scan got" is still an answer,
/// and refusing it was what this phase removed.
async fn info_offline(path: &Path, verbose: bool, map: bool, json: bool) -> Result<()> {
    let mode = CacheMode::Offline(path.to_path_buf());
    let (index, total_size) = match mode.load_offline().await? {
        CacheStatus::Valid { index, total_size, .. }
        | CacheStatus::Incomplete { index, total_size, .. } => (index, total_size),
        unusable => anyhow::bail!(unusable_cache_message(&unusable, path, None)),
    };
    report(&index, total_size, verbose, map, json);
    Ok(())
}

/// One column's resolution outcome, in both spellings: a stable token for
/// `--json` and the sentence `info --verbose` prints
/// (`docs/design/architecture.md`, "CLI surface").
///
/// **One match, two renderings.** Splitting them into two functions is how the
/// machine-readable export and the text listing drift into describing
/// different vocabularies; a single exhaustive match makes a new
/// [`ColumnResolution`] variant a compile error that has to answer both.
fn resolution_words(r: &ColumnResolution) -> (&'static str, &'static str) {
    match r {
        ColumnResolution::Mapped => ("mapped", "mapped"),
        ColumnResolution::UnknownType => {
            ("unknown_type", "unknown type — no mapping for this build")
        }
        ColumnResolution::NotDeclared => {
            ("not_declared", "not declared — no DDL explained this column")
        }
        ColumnResolution::MetadataNotScanned => (
            "metadata_not_scanned",
            "metadata not scanned — the scan never reached this database's DDL; finish the parse",
        ),
        ColumnResolution::OpaqueElementType => (
            "opaque_element_type",
            "opaque element type — the array's element type is information-free in the dump",
        ),
        ColumnResolution::NestedArrayElement => (
            "nested_array_element",
            "nested array element — the array's element type is itself an array",
        ),
        ColumnResolution::VaryingArrayShape => (
            "varying_array_shape",
            "varying array shape — dimensionality differs between rows, or a value carries an explicit lower bound",
        ),
        ColumnResolution::OpaqueBaseType => {
            ("opaque_base_type", "opaque base type — information-free in the dump")
        }
        ColumnResolution::EmptyEnum => ("empty_enum", "empty enum"),
    }
}

/// The sentence half of [`resolution_words`] — `info --verbose`'s per-column
/// line.
fn resolution_label(r: &ColumnResolution) -> &'static str {
    resolution_words(r).1
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

/// How a database is named in the listing. A `\connect`-less dump has no
/// name to print, and `(unnamed)` is what the listing calls that database —
/// one spelling, so the metadata header, the block listing and `--map` cannot
/// come to disagree about what an unnamed database is called.
fn database_label(database: &Option<String>) -> &str {
    match database {
        Some(name) => name,
        None => "(unnamed)",
    }
}

/// Prints a `database: <name>` line each time the database changes, and only
/// when a listing spans more than one — the common case (a plain or
/// single-`--create` dump) prints no header at all.
///
/// **The rows are already in file order and every `\connect` segment is
/// contiguous in the file**, so a header whenever the value changes is the
/// whole grouping rule: nothing has to be sorted or bucketed first. This is
/// also what makes an `AmbiguousTable` error's candidate names actionable —
/// they are names this listing already showed
/// (`docs/design/architecture.md`, "One target per query").
struct DatabaseHeadings<'a> {
    multi: bool,
    current: Option<&'a Option<String>>,
}

impl<'a> DatabaseHeadings<'a> {
    /// `databases` is every row's database, in listing order; only its
    /// cardinality is read here.
    fn new(databases: impl Iterator<Item = &'a Option<String>>) -> Self {
        let multi = databases.collect::<BTreeSet<_>>().len() > 1;
        Self { multi, current: None }
    }

    fn before(&mut self, database: &'a Option<String>) {
        if self.multi && self.current != Some(database) {
            self.current = Some(database);
            println!("database: {}", database_label(database));
        }
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
            println!("database: {}", database_label(&db.name));
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

/// One `COPY` block's resolved schema, paired back with the block it came
/// from — the single resolution pass `--verbose`'s text and `--json`'s export
/// both render (`docs/design/architecture.md`, "CLI surface"). Two passes is
/// the failure mode here: the export would quietly become a second
/// implementation of what the listing says.
///
/// `complete` says whether `index` covers the file
/// ([`DumpIndex::is_complete`]), which is what decides whether a block's
/// array-shape census may be believed. A *mapped* block's census is always
/// total for that block, but a reported schema answers "what is this table",
/// and one table's data can occupy several blocks (I2) — so a map that
/// stopped short cannot speak for a block past its frontier, and every column
/// resolves optimistically until it can
/// (`docs/design/architecture.md`, "The array shape census").
///
/// A header-less block resolves to an empty schema: its column names come from
/// its first data row, which no index records. It is still listed, so the
/// export's shape does not vary per block.
fn block_resolutions(
    index: &DumpIndex,
    complete: bool,
) -> Vec<(&pgdump_query::CopyBlock, ResolvedSchema)> {
    index
        .blocks()
        .map(|block| {
            let census: &[ArrayShape] = if complete { &block.array_shapes } else { &[] };
            let resolved = resolve_columns(
                &block.header.qualified_name(),
                &block.header.columns,
                index.metadata.as_ref(),
                block.database.as_deref(),
                SchemaMode::Typed,
                census,
            );
            (block, resolved)
        })
        .collect()
}

/// `--json`'s shape: the whole [`DumpIndex`] flattened to one object, plus the
/// three things it does not itself carry — how much of the file it covers, the
/// diagnostics `#[serde(skip)]` drops for the cache's own reasons
/// (`docs/design/architecture.md`, "The cache"), and the per-block type
/// resolution, which is an L2 conclusion an L1 index has no field for. No
/// schema stability is promised for any of this — see the `--json` flag's help
/// text.
///
/// **Coverage is components, not a rendered percentage.** `scanned_through`
/// comes flattened out of the index and `total_size` sits beside it, so a
/// script computes whatever ratio it wants instead of parsing the text
/// listing's line back apart.
#[derive(serde::Serialize)]
struct IndexJson<'a> {
    #[serde(flatten)]
    index: &'a DumpIndex,
    total_size: u64,
    diagnostics: &'a [Diagnostic],
    resolution: Vec<BlockResolutionJson<'a>>,
}

/// One `COPY` block's resolution, keyed by the block rather than rolled up per
/// table. A table can span blocks (I2) and a header-less block names its
/// columns from its first row, so a per-table rollup needs a merge rule that
/// does not exist yet; leaving the grouping to the consumer is where it
/// honestly sits (`docs/design/architecture.md`, "CLI surface").
#[derive(serde::Serialize)]
struct BlockResolutionJson<'a> {
    database: Option<&'a str>,
    table: String,
    header_offset: u64,
    columns: Vec<ColumnResolutionJson<'a>>,
}

/// One column's resolution: what the DDL declared, what it became, and why.
/// `arrow_type` is the exact string `info --verbose` prints for the same
/// column, so the two renderings cannot disagree about the type either.
#[derive(serde::Serialize)]
struct ColumnResolutionJson<'a> {
    name: &'a str,
    declared: Option<&'a str>,
    outcome: &'static str,
    arrow_type: String,
    plan: &'a NestedPlan,
}

fn print_index_json(index: &DumpIndex, total_size: u64, complete: bool) {
    let resolutions = block_resolutions(index, complete);
    let resolution = resolutions
        .iter()
        .map(|(block, resolved)| BlockResolutionJson {
            database: block.database.as_deref(),
            table: block.header.qualified_name(),
            header_offset: block.header_offset,
            columns: resolved
                .notes
                .iter()
                .enumerate()
                .map(|(i, note)| ColumnResolutionJson {
                    name: &note.column,
                    declared: note.declared.as_deref(),
                    outcome: resolution_words(&note.resolution).0,
                    arrow_type: arrow_type_label(
                        resolved.schema.field(i).data_type(),
                        &resolved.plans[i],
                    ),
                    plan: &resolved.plans[i],
                })
                .collect(),
        })
        .collect();
    let wrapped = IndexJson { index, total_size, diagnostics: &index.diagnostics, resolution };
    println!("{}", serde_json::to_string_pretty(&wrapped).expect("DumpIndex is always valid JSON"));
}

/// How much of the file the index covers, stated **once, at the top**, with
/// nothing below it qualified (`docs/design/architecture.md`, "CLI surface").
///
/// A partial index lacks *records*, not confidence: a block enters the map
/// only at a `CopyEnd` watermark and every mapping pass censuses, so every
/// record it holds is complete in itself. There is no half-known block, only
/// blocks past the frontier that are not there at all — which is why this line
/// is the only qualification the listing carries.
///
/// The percentage floors, so it reads 100% only for a genuinely finished scan.
fn completion_line(scanned_through: u64, total_size: u64) -> String {
    // A zero-byte file is trivially covered in full, and has no ratio.
    let percent = (scanned_through.min(total_size) * 100).checked_div(total_size).unwrap_or(100);
    format!("Scan completion: {percent}% ({scanned_through} bytes)")
}

/// Every `pgdq info` rendering goes through here: the coverage line, then the
/// listing or the export.
fn report(index: &DumpIndex, total_size: u64, verbose: bool, map: bool, json: bool) {
    let complete = index.is_complete(total_size);
    if json {
        print_index_json(index, total_size, complete);
        return;
    }
    println!("{}", completion_line(index.scanned_through, total_size));
    println!();
    print_index(index, verbose, map, complete);
}

/// Print the whole listing, below whatever coverage line [`report`] already
/// stated. `complete` is passed straight through to [`block_resolutions`],
/// which is where it means something.
///
/// **Nothing here is qualified by how much of the file was scanned.** The
/// coverage line above says it once; a partial index's records are each
/// complete in themselves (see [`completion_line`]), so repeating the caveat
/// per block would suggest a variation that does not exist.
fn print_index(index: &DumpIndex, verbose: bool, map: bool, complete: bool) {
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
        println!("{} span(s)", index.spans.len());
        return;
    }

    // One resolution pass, shared with `--json` — see `block_resolutions`.
    let blocks = block_resolutions(index, complete);
    if blocks.is_empty() {
        println!("no COPY blocks found");
        return;
    }

    let mut total_columns = 0usize;
    let mut total_unmapped = 0usize;

    let mut headings = DatabaseHeadings::new(blocks.iter().map(|(b, _)| &b.database));

    for (block, resolved) in &blocks {
        headings.before(&block.database);
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        if block.header.columns.is_empty() {
            println!("    columns: (not listed in COPY header)");
        } else {
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
    // No byte count here: the coverage line above owns that, and stating it
    // twice invites the two to disagree.
    println!("{} COPY block(s), {} row(s)", blocks.len(), index.total_rows());
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
/// "`DumpIndex`: one owner per fact"). Grouped by [`DatabaseHeadings`], the
/// same convention the ordinary block listing uses.
fn print_map(index: &DumpIndex) {
    let mut headings = DatabaseHeadings::new(index.spans.iter().map(|s| &s.database));
    for span in &index.spans {
        headings.before(&span.database);
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
