# P30.5 notes — one binary

What 30.6 inherits from deleting `datafusion-cli-pgdump`'s binary. The crate
is a library now (`datafusion-cli-pgdump/src/lib.rs`); `pgdt sql` is the only
way to run it, and its tests are `pgdt/tests/sql.rs` and the `sql` case of
`pgdt/tests/namespace_init.rs`.

## What 30.6 inherits

- **No figure can run yet.** The composed `pgdt` does not load in the
  register's image ([`../status/history/2026-10-05.md`](../status/history/2026-10-05.md),
  "The composed `pgdt` does not start in the register's image"), and every
  figure but the provider legs runs there. The sweep waits on the image the
  maintainer picks; STATUS's "Decisions worth another look" holds the
  question.
- **The provider legs run `/pgdt sql`, mounted where every leg mounts
  `/pgdt`**, in `Config.dfcli_image` as before: `measure.SQL_SHELL` is the
  program, `RunSpec.binary` `"dfcli"` now selects only that image, and
  `Session.binary_path("dfcli")` is `cfg.bin_pgdt`, so no second build is
  made or staged. The command names (`dfcli-dynamic-filter-…`,
  `dfcli-query-typed-jobs-…`), the reading keys and the `dfcli-introspect`
  target directory keep their spelling, so a past sitting's `raw.json` still
  renders.
- **The dynamic-filter figures now declare `pgdt/src/`**: the shell's
  dispatch and allocator are `pgdt`'s, so a change there moves them, and
  their marker goes red on any commit touching it.
- **`--profile-recipe`'s pair runs `target/profiling/pgdt sql`, and its
  introspection build is `pgdt --features introspect`**, whose report carries
  the allocator's sections before the evaluation section.

## Negative results

- **The tables no longer say the provider legs allocate with their own
  `mimalloc`**: they run the binary every other leg runs, so what departs is
  the image alone, and that is what each table states.
