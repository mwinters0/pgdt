//! `datafusion-cli-pgdump`: parses the CLI's arguments and hands them to the
//! library's entry point, upstream's `main` (`src/lib.rs`).

use std::process::ExitCode;

use clap::Parser;
use datafusion_cli_pgdump::Args;
use mimalloc::MiMalloc;

// The allocator is the binary's, never the library's (`docs/design/decisions.md`, "D13").
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    datafusion_cli_pgdump::run(Args::parse())
}
