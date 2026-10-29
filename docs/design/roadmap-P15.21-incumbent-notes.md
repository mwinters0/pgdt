# P15.21 notes — the default read as the incumbent

**The verdict is built; the readings remain.** `gzip-first-pass` now reads
its default, the physical count, as the incumbent
([`decisions.md`](decisions.md), "D2"): one file on which an upper count
beats it reopens D2's gzip clause, and the cap holds otherwise. Both re-takes
are owed from a commit carrying this verdict and the shipped 4 MiB chunk
(`measurements.md`, "A figure may be published outside the sweep"), so this
round stops before them.

## The bar, fixed before the reading

The bar is the harness's, so the table states the verdict
(`measure.gzpass_outcome`, `measure.gzpass_verdict`), by `gzpass_beats`'
pairs and margin, 7.1's bar:

- **`reopens`** is read first: the logical count or the logical count less
  one beats the physical count, on a file the pass lifts at that count — the
  physical count beating serial, or that upper count beating serial itself.
  One such file reopens the default, whatever the others read.
- **Otherwise each lifted file names the cost an upper count adds**, by 7.1's
  falsifiers: `shared`, `oversubscription`, or **`level`**, the physical
  count level with the logical count. `level` replaces 7.1's `stands`, which
  named the logical default standing; it no longer ships. The verdict lists
  each lifted file's outcome, and the pair table every file's.
- `no lift` and `regresses` are unchanged and settle nothing.

Applied to `ab7f630c`'s per-rep readings (`measurements.md`, "What a parallel
gzip first pass buys"), the bar reads zlib's and zlib-ng-compat's files
`shared`, GNU gzip's `level`, the level-1 file `no lift`, and the cap holding.
That checks the instrument; it is not a reading at the shipped chunk.

## What each verdict obliges

- **The cap holds**: D2 is unchanged. The prose under the table states each
  file's outcome, and `measurements.md`'s sentence saying the verdict line is
  read against the logical default goes.
- **It reopens**: the default count is the maintainer's. The round files it
  under STATUS's "Decisions worth another look" with the file and the pair
  cells, and the P34 inbox's physical-core entry, contingent on no reopening,
  goes with it.
- **`gzip-resident`**, by 15.18.3's bar: no cell under-billed, against the
  4 MiB chunk's charge (`Chunking::task_bytes`). An under-billed cell is a
  charge defect: the box stays open, and the defect is owed a `KD<k>`.

## The readings

From a clean tree at the commit carrying this change, detached per
`CLAUDE.md`, "Long-running processes", the sitting as 15.20's was
(`runs/gzip-first-pass-20261029-0143/HANDOFF.md`):

```sh
cd scripts && uv run measure.py --figure gzip-first-pass,gzip-resident
```

Both figures build the shipped binary in the release image; `gzip-first-pass`
also builds its 1 MiB leg (`gzip-1mib-chunks`). Before folding either, read
every column (`.claude/skills/evidence/SKILL.md`, rule 4).
