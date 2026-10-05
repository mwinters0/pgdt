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
    let code = datafusion_cli_pgdump::run(Args::parse());
    // The introspection build's report is the binary's too: its one section,
    // to the file the variable names, and nothing with it unset.
    #[cfg(feature = "introspect")]
    if let Some(path) = std::env::var_os(datafusion_cli_pgdump::INTROSPECT_OUT_VAR)
        && let Err(e) = std::fs::write(&path, datafusion_cli_pgdump::introspection_section())
    {
        eprintln!("introspect: cannot write {}: {e}", path.to_string_lossy());
    }
    code
}
