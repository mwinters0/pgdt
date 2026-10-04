# P31.21.2 — Where the strict listing's lines print: notes

What the slices after this one inherit. The listing is 31.21's
([`roadmap-P31.21-strict-unchecked-notes.md`](roadmap-P31.21-strict-unchecked-notes.md));
why it moved is
[`../status/history/2026-10-04.md`](../status/history/2026-10-04.md), "The
strict listing's lines print where they are asked for".

## What exists

- **`print_index` takes `unchecked`**, apart from `detail`: `report` passes
  its `--detail`, `parse` whether its mode was `strict`. The lines print
  only under it; every listing keeps the closing count, which says `each
  listed as` where they printed and points at `pgdt info --detail` where
  they did not. `--json` is untouched, carrying each block's `unchecked`
  whatever the flags.
- **What `info` prints depends on its own flag alone**, never on which run
  built the cache: a strict parse's cache read by `info` without `--detail`
  prints the count.
- **`--detail`'s help and the manual** ("`info`: reporting what is known",
  and type handling's strict paragraph) say where the lines print.
- **Evidence**: `pgdt`'s
  `a_strict_parse_names_what_it_leaves_unchecked_and_info_the_same`, now
  asserting a `default` parse and `info` without `--detail` print the count
  and no line, and `info --detail` the strict parse's lines exactly.

## Negative results and limits

- **No cache change**: the listing reads the preamble, so no
  `CACHE_FORMAT_VERSION` bump.
- **An `ignore` parse counts, as `default` does**: it promises nothing a
  listing of the unchecked would qualify.

## What the slices after this inherit

- **31.27's per-block partition line** prints where these lines print, being
  one of them.
