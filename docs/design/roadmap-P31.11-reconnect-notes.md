# P31.11 — A reconnect continues its database: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"What the register finds, and when the phase ends"; the entry it closes was
`KD73`.

## What exists

- **`dump_metadata_from_spans` continues the current database at a
  `\connect` naming it**, when no version-header pair is staged and no block
  has been read: that is the reconnect `RestoreArchive` writes after a
  `DATABASE PROPERTIES` entry, which I58 records. Every other `\connect`
  opens a database as before, so two concatenated dumps of one database stay
  two, as `target_settled` and `database_for_name` expect.
- **Nothing else moved**: the map already attributes every span and block by
  the governing `\connect`'s name, and the mapping pass restates metadata per
  name, so the one segment builder was the whole defect.
- **`CACHE_FORMAT_VERSION` was bumped**, both pins re-pinned beside it: the
  persisted metadata of the twelve fixtures holding a reconnect moved.
- **Evidence**: `tests/preamble.rs`'s
  `a_reconnect_after_database_properties_continues_its_database`, over
  `emitters/dumpall-binary-upgrade` and `emitters/dumpall-clean` at every
  major; unit tests `a_reconnect_to_the_current_database_continues_its_segment`
  and `a_connect_to_the_current_database_from_another_invocation_opens_a_segment`.
- **The known-failure table lost its `KD73` row** and, with it, its only
  `Resolves` case; what remains is `KD1`'s three rows.

## Findings

- **No `KD<k>` is owned by P31 any more**, and no register exemption names
  one, so the spec's end criterion holds with this slice: the phase is a wrap
  waiting.
- **A reconnect is told from a concatenation by I9's header pair, not by the
  name alone**: a hand concatenation lacking headers that repeats a database
  after its data still opens a second database, the data guard catching what
  the headers do not.
