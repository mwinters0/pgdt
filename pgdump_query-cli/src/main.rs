use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use pgdump_query::{ByteRangeSource, LocalFileSource};

#[derive(Parser)]
#[command(name = "pgdq", about = "Query pg_dump plain-format files without loading them into memory")]
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
            let size = source.size().await?;
            println!("{}: {size} bytes", file.display());
            if let Some(path) = cache_path {
                println!("(cache path override: {})", path.display());
            }
            println!("full-file scan / cache building not yet implemented");
        }
        Command::Info { file, cache_path, verbose } => {
            let _ = (cache_path, verbose);
            println!("{}: info not yet implemented (no cache reader yet)", file.display());
        }
    }
    Ok(())
}
