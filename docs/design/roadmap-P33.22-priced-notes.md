# P33.22 notes — 33.13.4's and 33.13.5's readers priced

**Three pairs are read, on legs that chain**: each pair's tree leg is the
next one's parent leg, the same binary, so their placements sum exactly
([`decisions.md`](decisions.md), "D108"). The logs are under
`runs/count-pair-33.22-20261109/` and `runs/count-pair-33.22-rework-20261109/`.

- **33.13.4, `89cf0a30` against `d33fa096`, resolved a rise on three rows**
  (`log-33.13.4`): the data-level parse, `query --schema-mode typed` and
  `pgdt sql -c`. The metadata-level parse and `query --schema-mode strings`,
  which read none of its readers, resolved nothing.
- **33.13.5, `f799618c` against `89cf0a30`, resolved a rise on the same
  three** (`pair-33.13.5/log` in the rework's directory), on 33.13.4's tree
  leg and the rework pair's parent leg, `49f3cf05`, whose `crates/`,
  `vendor/` and `Cargo.*` are `f799618c`'s: the largest change of the three.
  `log-33.13.5` is void (Negative results).
- **A rework of 33.13.4's readers, the tree against `49f3cf05`, resolved a
  fall on the data-level parse and a small rise on the typed query and
  `pgdt sql`** (`log`). It was never committed: P33's re-target reverts
  33.13.4 and 33.13.5 (33.23), so their rise is removed rather than reworked
  or accepted.

## Where each is placed

callgrind reproduces every row's change on both rep pairs, and the two agree
function for function: 33.13.4's in `callgrind-33.13.4/reading.txt`, 33.13.5's
and the rework's in the rework directory's `callgrind/`, as
`<row>-head-vs-89cf-place-<rep>.txt` and `<row>-rework-vs-head-place-<rep>.txt`.

- **33.13.4's is its own readers**: `KeyRead::byte` matching the reader's
  kind afresh each byte and calling `IntIn::byte` out of line, where
  `int_in` and `decode_uuid` read a slice; on the batch, `decode_uuid`
  feeding `UuidIn::byte` a byte at a time where the slice reader was inlined
  into `append_typed`.
- **33.13.5's is `NumericIn`**: `Plain::byte` the most of it, holding each
  digit in a `Vec<u8>` grown a push at a time, so the allocator's grow,
  realloc and free paths follow it per value; `NumericIn::whole`;
  `from_utf8` over the held digits where a `CharSearcher` ran, and
  `NumericIn::unscaled` building its `String`; `KeyRead::machine` and its
  drop. `typmod_round` moved into `input.rs` and costs what it did.

## What 33.23 inherits

**Both rises are placed in the push machines that replaced a slice reader**,
so putting the slice readers back removes them. No pair prices the revert,
by the maintainer's call on P33's re-target
([`../status/history/2026-11-10.md`](../status/history/2026-11-10.md)).
What the placements name is P40's
([`roadmap-P40-every-reader-streams-inbox.md`](roadmap-P40-every-reader-streams-inbox.md)).
The kept legs,
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.22/legs-{89cf0a30,49f3cf05}/`,
are read by nothing now and deletable.

## Negative results

- **`log-33.13.5` compared `89cf0a30` with itself**: its tree leg's build
  compiled nothing. The pair's target dir held the `89cf0a30` tree leg,
  built minutes before from another checkout, and cargo's freshness reads a
  source file's mtime against the last build's, so the `f799618c`
  checkout's files, written before that build, read as built. The rework
  pair's parent leg, the same crates, counted far more than that leg on
  every row 33.13.5's readers run on, and a recount of the kept `89cf0a30`
  leg the next day reproduced its logged count, so the machine had not
  moved. **`--count-pair` cannot tell
  a leg it built from one it reused**; a pair whose legs come from more than
  one checkout reads its build log for a `Compiling` line under each leg.
- **`--callgrind-recipe` had never run as printed**, and three faults each
  stopped or emptied it: the linked input's target was not mounted, a `pgdt`
  valgrind started was its PID namespace's init, whose handler on every
  ending signal valgrind refuses for the real-time one it reserves, and the
  placement's `awk` read no count `callgrind_annotate` right-aligns, every
  one but the widest. The recipe now mounts the input cache, starts each run
  under `sh`, and reads a count after blanks, each held by
  `test_measure.py`'s `CallgrindRecipe`.
- **A row's runs need not run one after another**: an `Ir` count does not
  move with what else the machine runs, so a placement's runs go four or
  five at a time, each with its own cache (`callgrind/run.sh`).
