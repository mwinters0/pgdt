# P30 — One binary for distribution: notes

The phase's notes, consolidated at its wrap. The spec is
[`roadmap-P30-one-binary.md`](roadmap-P30-one-binary.md); what it built is
[`../status/STATUS.md`](../status/STATUS.md)'s CLI and SQL-shell rows, why the
binary composes DataFusion's CLI and links mimalloc is
[`decisions.md`](decisions.md), "D13", and every figure it took is
[`measurements.md`](measurements.md)'s under the `1c9fc9be` stamp. This keeps
what none of them records: negative results, and where the facts aimed at
later phases went.

## The composition

- **The copied entry point is renamed, not kept as `main`**: `tokio-macros`
  2.7.2 refuses arguments on a function named `main` (`entry.rs`, "the main
  function cannot accept arguments"), so upstream's `#[tokio::main]` became
  `run(args)`, which builds its own runtime and must be called from outside
  any.
- **Upstream's `/// Calls [`main_inner`]` links a private item from a public
  one**, which `rustdoc::private_intra_doc_links` warns of under `cargo doc`.
  Nothing in the round's checks runs rustdoc and the line is upstream's, so it
  is left as written.
- **`concat!` cannot build the version string**: `DATAFUSION_CLI_VERSION` is a
  path to a constant, not a literal, so `main.rs`'s `version` formats the line
  once at run time.
- **The provider legs kept the `dfcli` spelling** — command names, reading
  keys and the `dfcli-introspect` target directory — though they run `pgdt
  sql`, so a sitting's `raw.json` from before the composition still renders.

## The instrument on mimalloc

- **mimalloc's `committed` at exit is retention, not what the run ended
  holding**: it purges after a delay, so a run exits with `committed` near its
  high-water (`runs/30.1-instrument-smoke/report.txt`). Its committed
  high-water is not a resident reading at a moment either; peak RSS less the
  live and glibc high-waters is the reading for retention.
- **An instrumented `pgdt sql` writes its report after `run` returns**, its
  runtime and threads gone; a native command's is written inside its runtime.
  Nothing in the gate reads a `sql` report.
- **The 128 MiB diagnostic legs are not more limits on
  `RESERVE_INSTRUMENT_LIMITS`**: that family feeds the counter's own line, a
  per-unit fit at `RESERVE_MECHANISM_UNIT`, so a 128 MiB leg there would put
  two units on one line.

## The gate and its sitting

- **The gate passed on the `1c9fc9be` sitting** (`runs/measure-20261005T155330/`):
  no leg of `reserve` or `parallel-peak-rss` was killed on either build, so
  `Session._gate_twin_of_kill` never ran and nothing failed on both legs.
- **The builds part by block size, unattributed**: the shipped build holds
  less than `system` on 24 MiB blocks and more on 128 MiB blocks wherever more
  than one reader runs, in `reserve` and `parallel-peak-rss` alike.
- **No twin is swept for `parallel-scan-throughput`**: it reads no resident
  set, so its gate reading is whether a leg was killed, which the kill twin
  reads when one is.
- **Three movements nothing attributes**: the warm and NVMe `INSERT`-run legs
  slowed against every earlier stamp, the sitting differing by the allocator,
  by DataFusion linked in and by `8cffbac`'s lexer, and the `allocator` figure
  timing no `INSERT` run; and a small `parse` holds more on the shipped build
  than on `system` (`rss-attribution`). Both are stated in `measurements.md`;
  a profile would rank the first's candidates and none was taken.
- **A margin verdict and a confirming option-off sitting were both tried and
  withdrawn**: the gate reads kills alone, the margin reported beside it
  ([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
  "P30's gate reads kills"; [`../status/history/2026-10-06.md`](../status/history/2026-10-06.md),
  "30.8's read-back goes with it"). `30f73fb6` holds the removed read-back.
- **`--render` re-runs three renderers' host steps**, so a re-render moves
  `nested-decode-micro`, `per-block-quadratic` and `preamble-prepass` cells
  that are no reading of the sitting; only `reserve`'s gate table and
  prose were re-folded, and `tables-as-taken.md` beside `tables.md` is what
  was folded first.

## Filed for later phases

- **P23**: the diagnostic sitting's two terms and mimalloc's candidates for
  the retained unit, in
  [its inbox](roadmap-P23-resident-reserve-inbox.md), "The unit mimalloc keeps
  of a 128 MiB block", beside the option read-back entry; the deficiencies are
  `KD34` and `KD111`.
- **P29**: the one artifact and its allocator, whether the release profile
  strips, and the composed binary's glibc floor, in
  [its inbox](roadmap-P29-releases-inbox.md).
