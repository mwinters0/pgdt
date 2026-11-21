# P43.7.1 — A re-read's policy as data: notes

Slice 43.7.1 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md),
earned by 43.7's review (`../status/history/2026-11-21.md`, "43.7's open,
reviewed"). No format change. The shape is D170, amended here.

## What the next slices inherit

- **`Policy::Rereads` carries a `Reread`**, in `open.rs` beside `Pass`, and
  exported with it as `pgdt_mapping::internal::Reread`. It has three arms:
  - `Gathers(&StatisticsBackfill, Gathering)`: the open builds
    `gather::observer_tracking` from the plan, under the gathering's account
    and `invalid`.
  - `Checks(Gathering)`: the open builds `gather::checker`, charging the
    gathering's account.
  - `Censuses`: no observer, `settle.rs`'s census of a block mapped at the
    metadata level.
- **The back-fill still decides which read to make.** `Rereads::reread`
  maps its `Option<&StatisticsBackfill>` to `Gathers` or `Checks`, and
  `reread_rows` takes the `Reread` in place of an observer.
- **`StatisticsBackfill` is declared `pub`**, its fields still `pub(crate)`
  and its module private, as `Gathering` is: a `pub` enum's arm holds it.
- **A re-read's observer is now built after `reread_rows`'s extent check**,
  not before it. A block refused as `CachedBlockChanged` there builds none,
  where it built and dropped one before; nothing else moves.

## Negative results

- **No `--count-pair`.** The slice changes where a re-read's observer is
  built, once per block, and builds the same observer with the same
  arguments; no row-path code changed.
