use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pgdump_query::cache::CacheMode;
use pgdump_query::{DumpIndex, LocalFileSource, ScanOptions, build_index};

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { file, cache_path } => {
            // `parse` is the eager entry point: always scan fresh (ignoring
            // any existing cache) and (re)write it, per `mvp.md`'s CLI spec.
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
                    // `mvp.md`'s "cache is never required for correctness"
                    // rule means this fallback must still produce a correct
                    // answer.
                    let source = LocalFileSource::open(&file)?;
                    let index = build_index(&source, &ScanOptions::default()).await?;
                    mode.save(&index)?;
                    index
                }
            };
            print_index(&index, verbose);
        }
    }
    Ok(())
}

fn print_index(index: &DumpIndex, verbose: bool) {
    if index.blocks.is_empty() {
        println!("no COPY blocks found in {} scanned bytes", index.scanned_through);
        return;
    }

    for block in &index.blocks {
        println!("{} ({} rows)", block.header.qualified_name(), block.row_count);
        if block.header.columns.is_empty() {
            println!("    columns: (not listed in COPY header)");
        } else {
            println!("    columns: {}", block.header.columns.join(", "));
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
