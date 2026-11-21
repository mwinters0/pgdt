# P43.5 — The observer's one-call close, and the gatherer as three: notes

Slice 43.5 of [`roadmap-P43-block-model.md`](roadmap-P43-block-model.md). No
format change, and no behaviour moved but what the pair prices.

## What the next slices inherit

- **`BlockObserver::close(self: Box<Self>, end) -> BlockObserved`** is the
  contract's close. It replaces `finish`, `take_ignored` and
  `checks_every_field`. `BlockObserved` (`pgdt_model::internal`) carries what
  the record takes: `gathered`, `ignored_refusals` and `checked_in_full`. Its
  two callers are `Builder::on_copy_end`, which fills a `BlockRecord` from it,
  and the back-fill's `Rereads::reread`, whose `BlockReread` holds it whole.
  43.6's `Recorder::finish` calls it once.
- **Three types implement the contract** in `statistics/gather/`:
  - `BlockGatherer` (`block.rs`) is a block that gathers. It holds the cap,
    the density bounds, the sizing it records and the live-pieces count, and
    its `absorb` downcasts to `PieceGatherer`.
  - `PieceGatherer` (`piece.rs`) is a piece. It holds its head and a clone of
    the block's live-pieces count, and it merges nothing.
  - `Checker` (`checking.rs`) is the strict checker. Its pieces are
    `Checker`s, and its `absorb` downcasts to its own type. It sits beside
    `strict.rs` rather than in it, since `read.rs` takes `FieldCheck` from
    there and the checker takes `Reader` from `read.rs`.
- **Two parts are shared, and each type composes them.**
  - `Reader` (`read.rs`) is what every observer reads a row by: the strict
    check and its walks, the field handed over in runs, the rows counted, the
    first refusal and the fields gone past. Its `fold` is generic over
    `Gather`, which `Groups` implements and `()` implements for the checker.
    So no method tests whether its observer gathers.
  - `Groups` (`gatherer.rs`) is what a block and a piece gather into: the
    groups, the columns, the charge, the merge and the decline.
    `close_through` takes a `Closing`, which is the block's merging cap or the
    piece's head.
- **What each type holds is charged where it is made.** `Groups::new`
  charges, and so `BlockGatherer::new` and the piece `BlockGatherer::piece`
  builds are each charged once. `Groups::carry` sets what a piece being
  folded in still holds and charges it in the same step. No caller sets a
  field and then calls `charge_held`. `observer_tracking` still calls
  `decline_if_over` after `BlockGatherer::new`, since the tests construct a
  block under a full account that must not decline before its pieces are
  made (`a_refusal_past_a_decline_counts_the_declined_rows`).
- **`checker` takes no `Sizing`**, and `Gatherer`'s `splits` field went: it
  was written and never read.

## Negative results

- **A piece's `close`, `piece` and `absorb` are `unreachable!`.** The
  contract stays one trait. A row-level trait for pieces under a block-level
  one would say this in the types, but 43.6's `Recorder` is one concrete type
  holding a block's observer or a piece's, which two traits would make
  generic or two types. 43.6 splits it, the `Recorder` generic over its
  observer (`roadmap-P43-block-model.md`, "One recorder, a held row split
  once").
- **The checker keeps a `Charge` that charges nothing.** It counts the
  checker among the observers the account may be short of. Without it, a
  strict parse's table of checks, which is attributed to statistics and
  charged to no account (D81), would fall outside the instrument's tolerance
  (`StatisticsAccount`, "What it does not see"). The redundant `set(0, 0)`
  the old constructor made after `Charge::new` is gone.
- **`ColumnGatherer::new` keeps `clippy::too_many_arguments`.** Its eight
  parameters are a resolved column's parts. The tests build one from a bare
  `ComparisonPlan` with no `ResolvedColumn`, so taking the resolved column
  would move the test fixtures, not the reshape this slice owns.
- **The data-level parse pays a few instructions a field and a row,
  accepted.** The `--count-pair` resolves a rise on the data-level parse and,
  by a few thousand instructions, on the strict parse over
  `long_escaped_32`; the metadata-level parse falls. Callgrind places both
  rises in the changed code. `Reader::fold` carries `&mut Groups` as one more
  argument, and the tree spills it on the fast path. `Reader::begin_row` is a
  call of its own where the parent's was inline. On the strict row,
  `FieldCheck::refused` is no longer inlined. Removing any of these would rest
  on codegen, which D108 refuses, and so would putting the groups inside the
  reader to drop the argument
  (`runs/count-pair-43.5-20261121/callgrind/reading.txt`).
- **A stopped piece's head is left as it was.** A decline or a refusal frees
  the piece's `Groups`, and the block drops that piece rather than joining
  it, so nothing reads the head again.
