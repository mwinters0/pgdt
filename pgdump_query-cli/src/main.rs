use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Parse { file, cache_path } => {
            let source = LocalFileSource::open(&file)?;
            let index = build_index(&source, &ScanOptions::default()).await?;
            print_index(&index, false);
            if cache_path.is_some() {
                println!();
                println!(
                    "note: --cache-path is accepted but the structure cache is not implemented yet"
                );
            }
        }
        Command::Info { file, cache_path, verbose } => {
            // No cache reader yet, so `info` scans the file itself. Once the
            // cache lands this reads the cache instead.
            let _ = cache_path;
            let source = LocalFileSource::open(&file)?;
            let index = build_index(&source, &ScanOptions::default()).await?;
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
