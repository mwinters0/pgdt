# P40 inbox — facts filed for its grilling

Evidence found in earlier phases that P40 (every reader streams) will need.
**This is a queue, not a document**: when P40 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## A push machine under `field_key` costs every short value

**Fact.** 33.13.4 and 33.13.5 replaced the slice readers `field_key` and a
query's typed decode read an integer, `oid`, `boolean`, enum, `uuid` and
`numeric` with push machines fed a `&str` a byte at a time. Each count pair
resolved a rise on the data-level parse, `query --schema-mode typed` and
`pgdt sql -c`. callgrind placed it in `KeyRead::byte`'s per-byte dispatch,
`IntIn::byte` and `UuidIn::byte` called out of line, and `NumericIn` growing
a `Vec<u8>` a digit at a time, with `from_utf8` and a `String` over the held
digits. 33.23 put the slice readers back.

**Why P40 cares.** Its central question is P33's "one grammar per type" against
a slice reader on the hot path. A machine run over a field's runs only where
the field spans one, beside the slice reader, is the alternative P33 rejected
as a second parser. A differential test, which every replaced reader already
has, would hold the two together.

**Origin.** 33.22, 2026-11-09 and 2026-11-10
([`roadmap-P33.22-priced-notes.md`](roadmap-P33.22-priced-notes.md));
`runs/count-pair-33.22-20261109/`, `runs/count-pair-33.22-rework-20261109/`.
The machines are at `89cf0a30` (33.13.4) and `f799618c` (33.13.5).

---

## What grows with each scalar's text

**Fact.** A float's blank, digit and `nan(…)` runs (I81); an IPv4 octet's
and netmask's leading zeros, the family decided by a `:` anywhere (I67);
`macaddr`'s `%x` runs and blanks (I68); a datetime's blank and punctuation
runs, which `ParseDateTime` skips without work-buffer space (I83);
`interval`'s ISO 8601 fallback, reading the raw text's digit runs with
`strtod` (I84); a `numeric`'s leading and trailing zeros; a range's bounds,
collected by `RangeWalk.bound`.

**Why P40 cares.** These are what each push machine carries as state rather
than holds. A datetime's machine was settled as `ParseDateTime`'s lexer alone,
its decoders and key reading the lexer's bounded fields, for a key, the tier
and a query's batch alike. Rejected there: the decoders as push machines; a
head clipped at `longest_read`, refusing a blank-padded value the server reads.

**Origin.** P33's grilling of 2026-11-09, which cut this work into
33.13.6–33.13.10 (`git show 49f3cf05:docs/status/STATUS.md`) and filed it as
D149, struck when P33 was re-targeted on 2026-11-10.

---

## A summed column and a container's tier still copy a long escaped field

**Fact.** `ColumnGatherer::observe` unescapes a long escaped field whole where
its column keeps a sum or counts unrepresentable values (D142): a sum needs the
integer's digits, and a container's tier walks `decode_*`, the output grammar.
No `pg_dump` writes an escape in such a column. Under P33 the copy is a parse's
collected scalar text, drawn from the ledger. A mapping pass's census holds a
counted container's field whole for the same reason (`FieldCount::keeps`,
`Kept::Whole`), so a cold query is charged for a long `timestamptz[]` field in a
table it reads beside it; the `*_in` walks a parse streams cannot stand in, reading
spellings `decode_*` refuses (`{ infinity }` in a `date[]`). A query's batch
does the same for a nested column under a tier test (D153), unescaping it and
decoding it through its elements.

**Why P40 cares.** It was 33.13.2: the output grammars' walk resumable, the
gatherer's sum and tier and the census's count read over it holding a bounded
state, a range's bounds fed as parts. The walk exists, pulling a field held
whole a byte at a time (`nested/output/walk.rs`, D151).

**Origin.** 33.13 and 33.13.1's notes; 33.13.2's row at `49f3cf05`; 33.20,
struck 2026-11-10.
