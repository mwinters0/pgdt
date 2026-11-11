# P33.21 notes — a whitespace run is rewound, not held

**An unquoted array element's whitespace run, and one after `NULL`, is fed on
into its part and rewound where it trails** ([`decisions.md`](decisions.md),
"D145"), so `ArrayWalk`'s state is fixed and `Blanks` is gone with `KD167`.
What proves it is `crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_check_fed_over_runs_holds_no_field`, whose rows now hold an
eight-MiB run alternating `' '` and `'\t'` interior, trailing and after `NULL`
either way, and a `json[]` element's `\v\t` run refused where interior; on
its parent the first of them fails, its peak growing with the run. No reading
was taken.

- **The protocol** (`value/source.rs`): `Parts::mark` before a run is fed on,
  `Parts::rewind` where it trails; after `NULL` the mark comes before the
  `begin`, so a rewind abandons the part and `null` follows. `ArrayWalk`
  holds the one bit the spec names, a refusal met in the run
  (`Stage::Unquoted { run }`, `Stage::NullBlanks { refused }`), standing only
  where the run turns out interior.
- **Who restores what**: `Collect` truncates; `Element` keeps a `Mark` of its
  `Check` (`grammar/walk.rs`) — each reader's fixed state, a collected text's
  length — taken by `Check::mark`; `JsonWalk`, `RangeWalk` and
  `MultirangeWalk` give their own (`JsonMark`, `RangeMark`,
  `MultirangeMark`). A nested container is restored to the part it was
  reading at the mark; a part it begins inside the run is readied again when
  begun after it.

## What 33.13.3 inherits

- **Every reader `Check` holds marks and rewinds**: a scalar reader replacing
  `Check::Scalar`'s collected text gives `Check::mark` its fixed state, and
  only whitespace reaches it between a mark and a rewind.

## Negative results

- **`benches/resumable/` keeps its own `Blanks`**: it is
  `resumable-reader-micro`'s apparatus, and editing it would stale the figure
  33.17 chose the form by.
- **No count pair was taken**: no `array_out` field holds an unquoted
  element's whitespace, so a dump's path is unchanged.
