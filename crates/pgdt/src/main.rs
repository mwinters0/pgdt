#![cfg_attr(not(feature = "introspect"), forbid(unsafe_code))]

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use namespace_init::InitShutdown;

mod alloc;
mod args;
mod info;
mod interrupt;
mod introspect;
mod parallel;
mod parse;
mod profile;
mod query;
mod refusal;
mod version;

#[derive(Parser)]
#[command(
    name = "pgdt",
    version = version::version(),
    about = "Query pg_dump plain-format files without loading them into memory"
)]
struct Cli {
    #[command(subcommand)]
    command: Invocation,
}

/// The two kinds of command, told apart before the process is set up: each
/// brings its own runtime, signal handlers and logging (`main`).
#[derive(Subcommand)]
enum Invocation {
    #[command(flatten)]
    Native(Command),
    /// A SQL shell over dumps `pgdt parse` has cached: DataFusion's own CLI,
    /// with each `--source` registered as catalogs, one per database it holds.
    ///
    /// It never parses: a dump is read through the complete cache `pgdt parse`
    /// leaves, and one without is refused naming the `parse` that builds it.
    /// `CREATE EXTERNAL TABLE … STORED AS PGDUMP` registers one table, and `SET
    /// pgdump.*` states a session's provider settings. Every flag but `--source`
    /// and `--strict-identity` is DataFusion's own, as its CLI ships it —
    /// `--memory-limit` among them, the limit on DataFusion's own memory pool,
    /// which the dumps' scans bill against the allowance they share.
    #[command(disable_version_flag = true, next_line_help = true, after_help = sql_release())]
    Sql(datafusion_cli_pgdump::Args),
}

/// The DataFusion release `pgdt sql` is, under its help.
fn sql_release() -> String {
    format!("DataFusion CLI v{}", datafusion_cli_pgdump::DATAFUSION_VERSION)
}

#[derive(Subcommand)]
enum Command {
    /// Scan a dump file and build the structure cache — the only command
    /// that scans ahead of what was asked (`pgdt info` reports from the cache
    /// this leaves; a `query` maps only what its table needs, but persists
    /// that as it goes unless told `--dtcache none`). Resumes from a matching cache rather than
    /// restarting, and banks its progress at `COPY` block boundaries as it
    /// goes — including on Ctrl-C, which saves what has been scanned and
    /// then ends the process by that signal, or with status 128+n as a
    /// container's init — so an interrupted scan is not
    /// wasted work. Remove the
    /// cache file to force a scan from byte 0.
    Parse(parse::ParseArgs),
    // Kept out of the help below: `docs/design/decisions.md`, "D61".
    /// Report what a dump's cache holds. **`info` never scans** — it reads the
    /// cache `pgdt parse` wrote and errors if there is not one, rather than
    /// starting an hours-long scan on your behalf. A cache from an
    /// unfinished scan is reported for as far as it got, with the coverage
    /// stated at the top.
    Info(info::InfoArgs),
    /// Stream a table's rows, optionally projected to named columns and
    /// filtered by a boolean expression over single-column predicates.
    Query(query::QueryArgs),
}

/// Wire the library's `tracing` facade to stderr — on by default, uniformly,
/// for `parse`, `info` and `query` alike, with no terminal detection
/// (`docs/design/decisions.md`, "D64").
///
/// One level, `INFO`, and no way yet to raise or lower it — `-vvv` and
/// `--quiet` are deferred and unallocated. RFC3339 timestamps
/// (`UtcTime::rfc_3339`) match the koji orchestrator's logs. No ANSI color,
/// and stderr rather than stdout, which carries `query`'s row data.
fn init_status_output() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .init();
}

/// **Parse first, then set up the process the chosen command needs.** Each
/// kind wants its own runtime and tokio refuses to start one inside another,
/// and `sql`'s handlers depend on its arguments (`datafusion_cli_pgdump::run`
/// installs them, the REPL leaving `SIGINT` uncaught to cancel a statement).
/// So nothing of [`commands`]' — its runtime, its init handlers, its status
/// output — exists while `sql` runs; installed there too, the init's `_exit`
/// on `SIGINT` would end the session a `ctrl_c` was cancelling a statement in.
/// As init, a signal before either arm's handlers is discarded
/// (`docs/design/runtime-invariants.md`, "RT19"), and that window is clap's
/// parse and the chosen arm's setup ahead of its handlers.
fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Invocation::Sql(args) => {
            // An `introspect` build's report: the allocator's sections and
            // the shell's own, written once `run` has returned.
            let _instrument = introspect::at_exit(introspect::Sections::AllocatorAndEvaluation);
            Ok(datafusion_cli_pgdump::run(args))
        }
        // Each library refusal says its remedy in `pgdt`'s flags, however
        // it reached here (`docs/design/decisions.md`, "D107").
        Invocation::Native(command) => {
            commands(command).map(|()| ExitCode::SUCCESS).map_err(refusal::spelled_chain)
        }
    }
}

/// `parse`, `info` and `query`.
///
/// **A `current_thread` runtime, not a multi-threaded one.** Every unit of
/// work this binary dispatches is a `spawn_blocking` task
/// (`docs/design/decisions.md`, "D12"), so the blocking pool tokio creates on
/// demand carries the scan and the process's thread count follows the
/// concurrency dispatched rather than the CPUs it can see. **Claimed as a
/// thread-count result, not a memory one**: an arena's retention is not
/// proportional to how many there are, so this does not on its own make the
/// process smaller and no reading here says it does.
///
/// The tasks [`interrupt::install_interrupt_guard`] cancels from are ordinary
/// `tokio::spawn` tasks and run on this thread: the scan loop awaits a
/// `spawn_blocking` join at every piece, so the runtime is parked in
/// `block_on` — driving the signal driver — whenever work is running.
#[tokio::main(flavor = "current_thread")]
async fn commands(command: Command) -> Result<()> {
    // A default build drops this entirely; an `introspect` build writes what
    // the process held to the file `PGDT_INTROSPECT_OUT` names, on the way
    // out (`src/introspect.rs`).
    let _instrument = introspect::at_exit(introspect::Sections::Allocator);
    // As its PID namespace's init alone, an exit on every signal but a fault
    // that ends `pgdt` elsewhere; `parse`'s guard takes `SIGINT` and `SIGTERM`
    // over (`docs/design/decisions.md`, "D26").
    let mut init = InitShutdown::install(&[]).context("installing the init's signal handlers")?;
    init_status_output();
    match command {
        Command::Parse(args) => parse::run(args, &mut init).await,
        Command::Info(args) => info::run(args).await,
        Command::Query(args) => query::run(args).await,
    }
}

#[cfg(test)]
mod tests;
