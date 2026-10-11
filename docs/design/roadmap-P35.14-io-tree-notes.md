# P35.14 notes — pgdt-io's tree

The cut toward [`roadmap-P35.11-trees-notes.md`](roadmap-P35.11-trees-notes.md)'s
"pgdt-io" tree. Every body moved whole; the crate's API and `internal` are
the names they were, so nothing above pgdt-io changed but comments citing a
path.

## Where the cut fell

- **The tree is the design's**, with three placements it left open:
  `budget_bytes` (a pool's budget from a caller's `Parallelism`, read by the
  plain source and by `XzBudget`) went to `source/pool.rs`;
  `PLAIN_PARTITION_CHUNKS` stayed with the plain source in `source/local.rs`;
  `DecodedBlock` and `BlockView` went with `BlockCache` to
  `source/xz/blocks.rs`.
- **Visibility is what `io`'s `pub(super)` already meant.** With every module
  a child of `io`, `pub(super)` was crate-wide. Now `source::{local, pool,
  xz}` and `xz::blocks` are `pub(crate) mod`, and `BufferPool`, `POOL_DEPTH`,
  `POOL_MAX_BYTES`, `PLAIN_PARTITION_CHUNKS`, `BlockCache` and `XzBudget` are
  `pub(crate)`, so the root modules' docs (`memory`, `parallelism`,
  `partition`) still link them and `parallelism`'s tests still name
  `POOL_DEPTH`. What `source/xz.rs` defines is private but
  `walk_seek_table`, its two sources being its children.
- **`tests.rs` split beside each module**, plus a `#[cfg(test)]`
  `test_support.rs` at the crate root for the fixtures several modules' tests
  build: a file to read, `BareSource`, the `xz` helpers, the synthetic seek
  tables and `FakeRoot`. The suite is the same 108 tests (107 without
  `introspect`). Three placements are not by file name:
  - the tests driving both `.xz` arms over one file (the two piecewise arms
    and the block arm alike, and the two check-owed tests with
    `corrupt_check` and `check_failed`) are in `source/xz/fetched/tests.rs`,
    whose rationale is that comparison with its local twin;
  - `XzSource`-driven tests of the budget split and the wait policy stay in
    `source/xz/file/tests.rs`, and so do the two discovery-section tests
    reading `XzSource`'s budget (`a_compressed_source_recommends_what_its_workers_hold`,
    `the_recommendation_an_allowance_is_solved_against_is_the_scans_own_charge`);
  - the cut-width test is in `source/xz/tests.rs` beside the advice, needing
    `BlockCache`, while `partition/tests.rs` holds the plain window and `cut`.
- **`measure.py`'s `READ` declares the fourteen production files** by path,
  the test files undeclared as `io/tests.rs` was.

## What later cuts inherit

- The cycles the design named are gone: `partition` reads
  `parallelism` and nothing reads back, the plain source and the block cache
  meet only in `pool`, and `fetched` reaches no item of `file`.
