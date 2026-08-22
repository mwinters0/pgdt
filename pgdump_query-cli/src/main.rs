use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use pgdump_query::{DumpIndex, LocalFileSource, ScanOptions, build_index, cache};

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
        #[arg(long)]
        cache_path: Option<PathBuf>,
    },
    /// Print what is known about a dump file from its cache.
    Info {
        file: PathBuf,
        #[arg(long)]
        cache_path: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
    },
}

/// `--cache-path`, or the colocated default (`<file>.dqcache`) when unset.
fn resolve_cache_path(file: &Path, cache_path: Option<PathBuf>) -> PathBuf {
    cache_path.unwrap_or_else(|| cache::colocated_path(file))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { file, cache_path } => {
            // `parse` is the eager entry point: always scan fresh (ignoring
            // any existing cache) and (re)write it, per `mvp.md`'s CLI spec.
            let path = resolve_cache_path(&file, cache_path);
            let source = LocalFileSource::open(&file)?;
            let index = build_index(&source, &ScanOptions::default()).await?;
            print_index(&index, false);
            cache::save(&path, &index)?;
            println!();
            println!("wrote cache to {}", path.display());
        }
        Command::Info { file, cache_path, verbose } => {
            let path = resolve_cache_path(&file, cache_path);
            let index = match cache::load(&path)? {
                Some(index) => index,
                None => {
                    // No usable cache: scan, then persist what we learned —
                    // `mvp.md`'s "cache is never required for correctness"
                    // rule means this fallback must still produce a correct
                    // answer, and there's no reason to throw away the scan
                    // we just paid for.
                    let source = LocalFileSource::open(&file)?;
                    let index = build_index(&source, &ScanOptions::default()).await?;
                    cache::save(&path, &index)?;
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
