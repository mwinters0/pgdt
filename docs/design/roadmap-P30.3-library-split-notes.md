# P30.3 notes — the library split

What 30.4 and 30.5 inherit from making `datafusion-cli-pgdump` a library. The
mechanism is `datafusion-cli-pgdump/src/lib.rs`, the upstream copy, whose
header says how it is re-applied at a major; `src/main.rs` is the binary.

## What 30.4 inherits

- **The entry point is `run(args: Args) -> ExitCode`**, upstream's
  `#[tokio::main] main` renamed and handed its arguments, so it builds its own
  multi-threaded runtime and must be called from outside any runtime: the
  `sql` arm calls it from a synchronous `main`, never from inside `pgdt`'s
  `current_thread` one. `tokio-macros` 2.7.2 refuses arguments on a function
  named `main` (`entry.rs`, "the main function cannot accept arguments"),
  which is why it is renamed rather than kept.
- **`run` does the `sql` arm's whole process setup**: `env_logger::init()`,
  the namespace-init handlers (`end_as_namespace_init(args.repl_mode())`) and
  the introspection build's exit report all stay inside it, after parsing, as
  the spec's "The process is set up after parsing" asks. The composed `main`
  has nothing of the arm's to install, and must not install `pgdt`'s
  `InitShutdown::install(&[])` before dispatching to it: that registers an
  `_exit` on `SIGINT`, which `signal-hook` would run beside the REPL's own
  `ctrl_c`, ending the session where the arm leaves `SIGINT` uncaught to
  cancel a statement (`namespace-init/src/lib.rs`, `InitShutdown::install`).
- **`Args` still carries `#[clap(author, version, about, long_about = None)]`.**
  As a subcommand's variant that `version` gives `pgdt sql` a `--version`
  printing this crate's name and version, which the spec refuses; whether it
  goes by a `pgdump:` line on the copy or by an attribute on `pgdt`'s variant
  is 30.4's, the second leaving the copy alone.
- **The binary declares the allocator.** `src/main.rs` holds the
  `#[global_allocator]`, the library none, so `pgdt` linking the library
  leaves `pgdt/src/alloc.rs`'s static the process's only one.
  `scripts/test_measure.py`'s check that this binary links mimalloc reads
  `src/main.rs`, and goes with the bin target in 30.5.

## Negative results

- **The copy's diff against upstream's 55.1.0 `main.rs` is the `pgdump:`
  lines and `rustfmt`'s rewrapping alone**: `diff` from the first `use` down,
  against `datafusion-cli/src/main.rs` in the release worktree
  `CLAUDE.local.md` names. The rename, the parameter, the removed
  `Args::parse()`, the `pub` on `Args` and the removed allocator each carry a
  marker.
- **Upstream's `/// Calls [`main_inner`]` now links a private item from a
  public one**, which `rustdoc::private_intra_doc_links` warns of under
  `cargo doc`; nothing in the round's checks runs rustdoc, and the line is
  upstream's, so it is left as written.
