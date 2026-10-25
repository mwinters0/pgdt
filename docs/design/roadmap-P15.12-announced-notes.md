# P15.12 notes — single-member gzip, the first pass announced

What a gzip first pass says on stderr, and where it says it.

- **`FirstPass` says it, in its constructors and its hand-over**: `fresh`
  and `spaced` log `first pass started, reading serially while it builds the
  index`, `resumed` the same with `resuming from a saved checkpoint` and the
  `checkpoint` and `plaintext` offset it resumes at, and `hand_over` logs
  `first pass complete, the index built` with `units` and `members` once the
  indexed source is built. Each names `source` — the origin as a message
  names it — and `codec=gzip`. The constructors take that name, so no pass
  can be built silent; both openers pass their origin's (D64's added clause).
- **A started line comes only after the open succeeds**: a resumed index
  `resumed` refuses (`Recognized::Mismatch`) says nothing, and a hand-over
  whose indexed source fails to build says no complete line.
- **One pair on a cold `parse`**: the source is opened once, the preamble
  pass and the scan reading through it, so the complete line falls wherever
  the first read meets the end — inside the preamble pass on a file under
  one chunk. A `parse` or `info` over the complete cache says nothing of it.
- **The evidence**: `crates/pgdt/tests/gzip_source.rs` — a cold `parse` of a
  `-Z` fixture says one started and one complete line, in that order, and a
  second `parse` and an `info` none; a cold `query` stopped at a small table
  says started and not complete, and the `parse` resuming its cache names
  `checkpoint=0 plaintext=0`, then completes.

## What later slices inherit

- **15.8 rewrites the started line's `reading serially`** where the pass is
  no longer serial; the text is in `FirstPass::spaced` and `resumed`.
- **15.6 announces the BGZF walk** at open, as the `.xz` walk is, which this
  slice does not touch: a BGZF file opened over its walk is not a first pass.
- **The manual** says nothing of these lines yet; 15.7.2 writes them into
  `dump-inspection.md`'s "Status on stderr" beside the `.xz` walk's.

## Negative results

- **The constructors take a `String`, not the `Origin`**: naming
  `source::origin` from `first_pass` closes a module cycle through the
  openers that build the pass, which `module_graph.py` refuses.

- **No library test sees the lines**: `pgdt-io`'s tests install no
  subscriber, as `status_output.rs`'s module doc says of the `.xz` walk, so
  the binary's test is where the lines are asserted.
