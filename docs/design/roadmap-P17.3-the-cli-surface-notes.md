# P17.3 — the CLI surface

What the phase wrap, and anything that touches these messages, inherits.

## Two refusals, one tail

A user meets "this cache was written from another file" through two conditions,
and after this slice both end in the same clause — **`— remove it, or name a
different cache path`**:

- the stored-size mismatch, raised by the library as
  `Error::CacheSourceMismatch` and printed verbatim by `parse` and `query`;
- a compression claim the file contradicts, caught by recognition before a
  source exists and refused by `open_with_cache` for all three commands.

`refusals_name_both_ways_out` (`pgdump_query-cli/tests/partial_reporting.rs`) is
what holds them to one wording, the two sentences living in two crates with no
shared constant between them — `error.rs`'s is inside a `#[error(…)]` attribute
and the CLI's is `main.rs`'s `TWO_WAYS_OUT`.

## What changed, and the one thing that did not

**`parse`'s size-mismatch message is the library's, unchanged.** D7 asks for
what it found, what it expected and the two ways out; the library sentence
already carries all three, so nothing in the CLI re-words it. The rejected
alternative — catching the error and re-rendering it so `parse` also names the
dump path — is beside the mechanism (`architecture.md`, "The CLI's two refusals
are worded as one"), and the short of it is that the dump is the argument the
user just typed.

**`info` needed it, and the change is an ordering.** Its `SourceChanged`
sentence ended in `run \`pgdq parse --source …\``, which 17.2 falsified without
touching: `parse` refuses that same condition, so a reader following the advice
met a second refusal. It now names the two ways out first and the command
after. The other three `CacheStatus` arms are unchanged and still end in `pgdq
parse` on its own, because `parse` scans over all three.

**`query` needed nothing.** It surfaces the library error for the size
mismatch and the shared sentence for the compression contradiction.

**The contradicted compression claim stopped borrowing `CacheStatus::Unreadable`'s
sentence**, which 17.2's notes left for this slice to weigh. Two things were
wrong with the borrow once `parse` refused rather than rescanned: "check the
path, or run `pgdq parse --source …`" is advice the command that just refused
cannot take, and the bytes at that path *are* a pgdq cache — for another file.
It is a free function, `cache_written_for_another_file`, not a fifth arm of
`unusable_cache_message`: that match is exhaustive over `CacheStatus` so a new
status must be answered, and this condition is not a status.

**`open_with_cache` now answers the source or bails**, rather than handing
`Recognized` back to three call sites that each wrote the same refusal. Only
`CacheMode::Enabled` claims anything, so only it can be contradicted, and it is
the mode holding the path the message names — the refusal has everything it
needs where it stands.

## What the wrap inherits

- **The phase's slices are all landed.** Nothing in the checklist is short.
- **`.xz` still pays a footer walk before the size-mismatch refusal**, which is
  the one thing 17.2 handed here that this slice did not take. It is out of this
  slice's spec row, and taking it means deciding where "the cache at this path
  records another file's stored size" is answered *before* a source is opened —
  `known_compression` collapses every unusable outcome to
  `KnownCompression::Unknown` today, deliberately. Admitted as `M62` with its
  Blocks column empty: it costs one wasted walk on an error path (85 s for the
  koji download, one read for every other shape), and no slice of this phase is
  built around it.

## Evidence

- `refusals_name_both_ways_out` — the three commands against a grown dump: each
  refusal names both ways out, and `info`'s names them before `pgdq parse`.
- `every_command_refuses_a_cache_that_does_not_describe_the_file`
  (`tests/xz_source.rs`) now asserts the condition's own sentence and asserts
  that `is not a pgdq cache` is *not* what it says.
- `info_against_a_changed_file_says_the_file_changed` and
  `parse_refuses_a_cache_that_records_another_source_and_leaves_it_alone` stand
  unchanged, which is what says the ordering change did not move either fact.

## Claims corrected in this change

`docs/manual/dump-inspection.md`'s "When `info` says it cannot answer" listed
four messages and said every one ends in `pgdq parse`; it lists five now and
says which two do not, since that split — is the file at the cache path worth
keeping — is the fact a reader needs. `architecture.md`, "The cache" carried
the same "all four end in `pgdq parse`" sentence, and "The compressed source"
called `Recognized::Mismatch` reportable in `CacheStatus::Unreadable`'s
vocabulary; both are corrected, and the wording rule itself is a new subsection
under "CLI surface".
