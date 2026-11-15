# P33.4.6 notes — the charge counts only lines the read can hold

**`Table::line_charge` passes over a block the read holds no row of**: one
whose every run of groups the static filter prunes (its kept runs all the
empty header run, `prune.rs`'s `or_header_run`), and one whose batch decodes
no column, whose static filter reads no term and which holds no sorted stop
(`PlannedBlock::reads_fields`, beside `Pruned::stops`). The replay's
`reads_no_field` no longer asks a dynamic filter, so a long row of such a
block is streamed and kept unread whatever the state reads (D138, D160).

## What the next slices inherit

- **A `count(*)` and `--no-columns` read charge nothing**, so 33.5's floor,
  holding `Table::line_charge` against what the cap leaves `at(1)`, never
  refuses one, as the spec's "A pass holds a line only where it decodes it"
  calls it flat.
- **The charge's test and the replay's are two statements of one rule**:
  `line_charge` in `stream/replay/plan.rs` reads the planned block, and
  `reads_no_field` in `stream/replay.rs` the batcher and filter `activate`
  built from it. A block the plan resolved differently from what its header
  turns out to be is resolved again by `activate`, where the two could part;
  no dump in the tests does.
- **A streamed row still draws once, briefly**: the carry's
  `charge_absorbing` is an upper bound at the chunk where a row starts
  streaming — what it carries of the line plus the next chunk, at most two
  chunks — drawn before the carry knows it will stream, and returned at the
  next chunk. Nothing carves it now that such a read's charge is zero; it is
  admitted past the room where no other line holds (D157). 33.5's floor
  meets it as a line one reader holds within twice its chunk.
- **What is billed falls with it**: a provider scan reading no field bills
  later scans nothing (33.4.5's `held`).

## Negative results

- **A kept run's own longest line is not bounded**: the map records one
  line per block, so a read keeping any group of the block holding its
  longest line is charged it, whichever group holds it.
- **The pruning is planned twice**, by `line_charge` and by
  `ReplayPlan::new`, as the schema already was; neither reads a byte.
- **The held row under a dynamic filter is gone as a contract**:
  `dynamic_filter.rs`'s test that a state reading a field held a no-field
  plan's long row and dropped it is now
  `a_row_a_plan_reading_no_field_streams_is_kept_whatever_the_state_reads`,
  short rows still evaluated and dropped beside the long rows kept.
- **No count pair**: the replay's per-chunk streaming test lost a term and
  no per-row or per-byte path changed (D108).
