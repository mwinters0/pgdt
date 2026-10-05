# P30.4 notes — `pgdt sql`

What 30.5 and 30.6 inherit from composing the DataFusion CLI into `pgdt`. The
mechanism is `pgdt/src/main.rs` (`main`, `Invocation`, `commands`, `version`)
and `pgdt/src/introspect.rs` (`Sections`); the library side is
`datafusion-cli-pgdump/src/pgdump.rs` (`DATAFUSION_VERSION`,
`introspection_section`).

## What 30.5 inherits

- **`pgdt sql` is `datafusion-cli-pgdump`'s binary, with nothing of `pgdt`'s
  set up around it**: no `InitShutdown`, no tracing subscriber, no
  `current_thread` runtime — `run` builds its own runtime, logging and
  handlers, as the standalone binary's `main` has it do. So a test moved to
  `pgdt/tests/` against `pgdt sql` should see what it saw against the old
  binary, stdout, stderr and exit code alike. Nothing in this slice ran the
  moved tests; the smoke run was `pgdt sql -c` over a parsed fixture.
- **`run` no longer writes the introspection report.** The section is
  `introspection_section()`, written by whichever binary owns the process:
  `datafusion-cli-pgdump`'s `src/main.rs` writes it alone, `pgdt sql` appends
  it to the allocator's sections, under `evaluation_scope`. When the bin target
  goes, so does the first writer; `INTROSPECT_OUT_VAR` stays exported and
  `test_measure.py`'s `test_the_variable_matches_the_binarys` still reads
  `pgdump.rs`. `measure.py --profile-recipe`'s `dfcli-introspect` build is
  the standalone binary's and moves to `pgdt --features introspect` running
  `sql`.
- **The library exports three things**: `Args`, `run`, and
  `DATAFUSION_VERSION` (plus the two introspection items under the feature).
  `DATAFUSION_VERSION` is a constant of `pgdump.rs` re-exported, not a `pub
  use` of `datafusion_cli::DATAFUSION_CLI_VERSION` from `lib.rs`, whose
  upstream `use` already imports that name.
- **`sql`'s help is shaped by `pgdt`'s variant attributes, not by the copy**:
  `disable_version_flag` (the copy's `version` would print this crate's name
  and version), `next_line_help` (`--dump`'s value name is wider than a 100-
  column page, which otherwise puts every other flag's help one word to a
  line), and `after_help` naming the DataFusion release. The copy's `Args`
  attribute is unchanged.

## What 30.6 inherits

- **`pgdt` now links DataFusion in every build**, `--no-default-features
  --features system` included, and the build carries `tokio/rt-multi-thread`
  (`docs/design/runtime-invariants.md`, "RT17", whose re-verify now reads
  `pgdump_query`'s tree). No figure has been taken on the composed binary.
- **An instrumented `pgdt sql`'s report is written after `run` has
  returned**, its runtime and worker threads already gone; a native command's
  is written inside its runtime. The two-heap readings are process-wide either
  way; nothing in the gate reads a `sql` report.

## Negative results

- **`concat!` cannot build the version string**: `DATAFUSION_CLI_VERSION` is a
  path to a constant, not a literal, so `alloc.rs` keeps the allocator's
  markers as a constant and `main.rs`'s `version` formats the whole line once.
- **The help-width test now skips a line holding only an option's spec**,
  which clap never breaks; `--dump`'s is the only one wider than the page.
