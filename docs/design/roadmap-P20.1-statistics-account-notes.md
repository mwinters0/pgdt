# P20.1 — The statistics account: notes

The account is `statistics::StatisticsAccount`, each observer's share a
`statistics::Charge`, read whole as `MapRun::statistics` and printed by every
`parse` holding statistics as its `statistics held` status line
([`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--statistics`:
what `parse` records for later queries"); why it is shaped that way is
[`decisions.md`](decisions.md), "D81", and the layouts it rests on are
[`runtime-invariants.md`](runtime-invariants.md), "RT11" and "RT12". The
instrument that checks it is `pgdump_query::instrument` behind the library's
`introspect` feature, which `pgdump_query-cli`'s own `introspect` feature turns
on; its readings are the `statistics_*` lines of that build's report.

## The reconciliation

`pgdump_query-cli/tests/statistics_account.rs` is the reading and the check at
once: a table of reasonable width and one of sixty-four text columns, each
through a serial leg at a stated group size, the same split across four
workers, a back-fill of the first leg's cache and a flagless leg; and sixty-four
columns of distinct values at `STORED_VALUE_CAP`, flagless, whose open groups
grow a stored value a row. Its tolerance is in its module doc, registered before
the reading: at every update and at the peak, either way, a step for each open
observer plus the input's longest row; at return, the longest row. Every leg
passes; the sitting's readings are
`runs/statistics-account-20260915/readings-step.txt`, beside the first
tolerance's in `readings.txt`.

`pgdump_query/tests/statistics_heap.rs` holds the retained and loaded terms
exactly, per block, under `cargo test --workspace`, and `gather.rs`'s
`a_vector_grows_into_the_capacity_it_is_charged` holds a vector's charge ahead
to the capacity it grows into.

## Negative results

- **Charging a close's growth when the close ends fell short at the peak.** An
  interning map rehashes by allocating its larger table while the smaller is
  still live; a map about to grow is charged its larger table before the
  insert.
- **An open group charged at the largest a close had measured** left a
  column's first group uncharged and a later, larger one unseen, and updates
  came only at closes. Rows now report their column's growth, O(1) a field, and
  an observer updates once it passes `CHARGE_STEP`.
- **A vector pushed before it was charged** was the candidate for the shortfall
  that scaled — 2.1 MiB on the first tolerance's reasonable split leg: a
  close's `null_counts`, bounds and dictionary pushes, and a fold's appends,
  realloc'd while another column's or piece's update read the heap before the
  pusher's own. Every per-group and per-entry vector now grows through
  `reserve_charged`, and with the step and the fold below the leg's worst
  shortfall sits inside its steps; how much of the fall each of the three
  carried was not separated.
- **A fold raising the block before releasing the piece** read the moved groups
  twice at the peak. The piece's charge now moves onto the block's in one
  update, and the block charges what the piece still holds as its parts move
  across or are freed.
- **An allocation announced ahead reads as not yet made on another thread.**
  The first split leg under the new tolerance read 12.1 MiB short past its
  steps: one piece's map rehashing between its announcement and its release,
  the new table live, while another piece's update subtracted the announcement.
  The check now judges a shortfall against the whole account and an excess
  against the account less what is announced — see
  [`decisions.md`](decisions.md), "D81".
- **A check could read another thread's update half applied.** One run of two
  read a split leg 0.7 MiB over past its allowance, an announcement counted in
  the terms and not yet in the announced bytes. The account is now one lock
  over its terms in every build, an update, its peak and the check read whole,
  and three runs agree.
- **Checking every charge's open and close found two windows no earlier update
  had read.** A fold dropped the piece's remnant — its interning maps — while
  the block's charge still carried it, 44 MiB over; the remnant now goes back
  to the piece's own charge in one update, released as the piece is freed. And
  a block observer's column resolution was live, uncharged, at both its
  charge's opening and its first update, 11 KiB short past a wide table's
  allowance; the charge now opens first and the scratch is freed before the
  first update.
- **The back-fill legs' difference at return was the instrument's, not the
  account's.** The CLI's cache claim decoded every block and freed each `Arc`
  outside a statistics scope, and a re-read block's observer freed its own
  allocation as `finish` returned, outside one too. Both now free inside a
  scope, and the back-fill legs agree exactly at return.
- **A back-fill at twice the group size gathered almost nothing** on the
  reasonable shape: a group twice as wide holds more distinct texts than a
  dictionary keeps. The leg re-reads at half the size instead.

## What the next slices inherit

- **A merge (20.3, 20.4) rebuilding a block's vectors grows them through
  `reserve_charged`**, or the account is short of the coarser vectors beside
  the finer ones for as long as the merge runs.
- **A fold is exact only because pieces fold with none gathering**
  (`crate::leader`'s window): the block's charge carries the piece's remainder
  between its own updates, and nothing else updates the account meanwhile.
- **A block's interning maps can outweigh what the block retains.** On the
  reasonable shape the interned term's peak stood above the retained one, so a
  gathering block holds its statistics plus its dictionaries' second copy until
  it finishes. The decline (20.7) reads gathering and interned together, never
  a retained estimate.
- **What the account does not see**, each named in its rustdoc: each open
  observer's growth under a step, a row's decode scratch and the observer's own
  allocation.
- **Only a mapping pass keeps an account.** A `query` loads the cache whole,
  charges nothing and prints no `statistics held` line; the reserve's `query`
  legs (20.8) need a term of their own, the walk being
  `stream::statistics_heap`.
- **What the account costs a gathering `parse` is unpriced.** Every tracked
  field reads its open group's heap twice, and every group close recomputes the
  observer; `statistics-gathering` is re-taken in 20.9, and `measure.py
  --stale` names it red until then.
