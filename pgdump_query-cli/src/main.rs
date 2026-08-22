use std::ops::ControlFlow;
use std::path::PathBuf;

use anyhow::{Context, Result};
use arrow::array::{Array, RecordBatch, StringViewArray};
use clap::{Parser, Subcommand};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    BatchOptions, DumpIndex, DumpMetadata, LocalFileSource, Predicate, PredicateOp, ScanOptions,
    build_index,
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
        /// Single-column filter: `column=value` or `column!=value`, compared
        /// against each row's decoded field value.
        #[arg(long)]
        filter: Option<String>,
    },
}

/// Parse a `--filter` argument into a [`Predicate`]. `!=` is checked before
/// `=` since it contains that byte.
fn parse_filter(spec: &str) -> Result<Predicate> {
    let (column, op, value) = match spec.split_once("!=") {
        Some((column, value)) => (column, PredicateOp::Ne, value),
        None => match spec.split_once('=') {
            Some((column, value)) => (column, PredicateOp::Eq, value),
            None => {
                anyhow::bail!("--filter must be `column=value` or `column!=value`, got `{spec}`")
            }
        },
    };
    Ok(Predicate { column: column.to_string(), op, value: value.to_string() })
}

/// Print one batch's rows tab-separated, `\N` for NULL — mirroring COPY
/// TEXT's own NULL marker.
fn print_batch(batch: &RecordBatch) {
    let columns: Vec<&StringViewArray> = batch
        .columns()
        .iter()
        .map(|c| {
            c.as_any().downcast_ref::<StringViewArray>().expect("query columns are all Utf8View")
        })
        .collect();
    for row in 0..batch.num_rows() {
        let fields: Vec<String> = columns
            .iter()
            .map(|c| if c.is_valid(row) { c.value(row).to_string() } else { "\\N".to_string() })
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
            print_index(&index, false);
            pgdump_query::cache::save(&path, &index)?;
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { file, cache_path, verbose } => {
            let mode = CacheMode::resolve(&file, cache_path.as_deref());
            let index = match mode.load()? {
                Some(index) => index,
                None => {
                    // No usable (or disabled) cache: scan, then persist what
                    // we learned — a no-op under `CacheMode::Disabled` —
                    // `roadmap-phase1-mvp.md`'s "cache is never required for
                    // correctness" rule means this fallback must still
                    // produce a correct answer.
                    let source = LocalFileSource::open(&file)?;
                    let index = build_index(&source, &ScanOptions::default()).await?;
                    mode.save(&index)?;
                    index
                }
            };
            print_index(&index, verbose);
        }
        Command::Query { file, table, cache_path, filter } => {
            let mode = CacheMode::resolve(&file, cache_path.as_deref());
            let predicate = filter.as_deref().map(parse_filter).transpose()?;
            let source = LocalFileSource::open(&file)?;
            let mut header_printed = false;
            let mut rows = 0u64;
            pgdump_query::read_table(
                &source,
                &table,
                &ScanOptions::default(),
                &BatchOptions::default(),
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

/// Look up `column`'s declared type string for `qualified` table, from
/// whichever database's metadata explains it. `info` is diagnostic output,
/// not a query: a table name landing in more than one database's `tables`
/// map (only possible in a multi-database dump — untested, see
/// `docs/design/roadmap-phase2-typed-columns.md`, "Multi-database dumps") is
/// resolved by just taking the first match rather than erroring, unlike a
/// real query would. Associating each `CopyBlock` with the database it
/// belongs to, so this can't happen, is Phase 2.3's `resolve.rs`.
fn declared_type<'a>(
    metadata: &'a Option<DumpMetadata>,
    qualified: &str,
    column: &str,
) -> Option<&'a str> {
    let metadata = metadata.as_ref()?;
    metadata.databases.iter().find_map(|db| {
        db.tables.get(qualified).and_then(|cols| {
            cols.iter().find(|(name, _)| name == column).map(|(_, ty)| ty.as_str())
        })
    })
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

fn print_index(index: &DumpIndex, verbose: bool) {
    if let Some(metadata) = &index.metadata {
        print_metadata(metadata);
        println!();
    }

    if index.blocks.is_empty() {
        println!("no COPY blocks found in {} scanned bytes", index.scanned_through);
        return;
    }

    for block in &index.blocks {
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        if block.header.columns.is_empty() {
            println!("    columns: (not listed in COPY header)");
        } else {
            let qualified = block.header.qualified_name();
            let columns: Vec<String> = block
                .header
                .columns
                .iter()
                .map(|c| match declared_type(&index.metadata, &qualified, c) {
                    Some(ty) => format!("{c} {ty}"),
                    None => format!("{c} (unknown)"),
                })
                .collect();
            println!("    columns: {}", columns.join(", "));
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
        index.blocks.len(),
        index.total_rows(),
        index.scanned_through
    );
}
