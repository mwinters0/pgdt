# P40 inbox — facts filed for its grilling

Evidence found in earlier phases that P40 (every reader streams) will need.
**This is a queue, not a document**: when P40 is grilled, walk every entry,
fold it into the spec or discard it as stale, and delete this file. See
`docs/process.md`, "Inboxes: facts filed by destination".

---

## A push machine under `field_key` costs every short value

**Fact.** Two landed slices replaced the slice readers `field_key` and a
query's typed decode read an integer, `oid`, `boolean`, enum, `uuid` and
`numeric` with push machines fed a `&str` a byte at a time. Each count pair
resolved a rise on the data-level parse, `query --schema-mode typed` and
`pgdt sql -c`. callgrind placed it in `KeyRead::byte`'s per-byte dispatch,
`IntIn::byte` and `UuidIn::byte` called out of line, and `NumericIn` growing
a `Vec<u8>` a digit at a time, with `from_utf8` and a `String` over the held
digits. A cheaper rework — `KeyRead::run` picking its machine once per run,
`IntIn` reading by table into a saturated `u64`, `UuidIn` without per-byte
flags — recovered the data-level parse but rose on `query --schema-mode typed`
and `pgdt sql -c`; it was never committed, its one build kept at
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.22/legs-49f3cf05/pgdt-count-pair-tree`.
A later commit put the slice readers back (D144).

**Why P40 cares.** Its central question is "one grammar per type" (D144)
against a slice reader on the hot path. A machine run over a field's runs only
where the field spans one, beside the slice reader, is the alternative rejected
there as a second parser. A differential test, which every replaced reader
already has, would hold the two together.

**Origin.** Count pairs of 2026-11-09 and 2026-11-10,
`runs/count-pair-33.22-20261109/`, `runs/count-pair-33.22-rework-20261109/`.
The machines are at `89cf0a30` and `f799618c`, their differential suites with
them.

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

**Origin.** The grilling of 2026-11-09, which cut this work into slices
(`git show 49f3cf05:docs/status/STATUS.md`) and filed it as D149, struck when
the work was re-targeted on 2026-11-10.

---

## A summed column and a container's tier still read a long field whole

**Fact.** `ColumnGatherer::observe` unescapes a long escaped field whole where
its column keeps a sum or counts unrepresentable values (D142): a sum needs the
integer's digits, and a container's tier is read by `decode_*`'s grammar, the
output grammar's walk testing each leaf as it reads it and staging none (D153).
No `pg_dump` writes an escape in such a column. The copy is a parse's
collected scalar text, drawn from the ledger. A mapping pass's census holds a
counted container's field whole for the same reason (`FieldCount::keeps`,
`Kept::Whole`), walked off its bytes with no copy, so a cold query is charged
for a long `timestamptz[]` field in a table it reads beside it; the `*_in`
walks a parse streams cannot stand in, reading spellings `decode_*` refuses
(`{ infinity }` in a `date[]`).

**Why P40 cares.** It was a planned slice: the output grammars' walk resumable, the
gatherer's sum and tier and the census's count read over it holding a bounded
state, a range's bounds fed as parts. The walk exists, and the tier reads
through it, over a field held whole (`nested/output/walk.rs`,
`unrepresentable.rs`'s `tier_walked`).

**Origin.** The slice row at `49f3cf05`'s `docs/status/STATUS.md`, and the
re-target of 2026-11-10.

---

## What else holds whole, or reads by slices of its own

**Fact.** A TimescaleDB catalog block the scanner records holds every row whole
(`record_rows`, D133), never streaming. A strict check of a walked container
collects each scalar part's text and a range's bounds through the global
allocator, no ledger drawing for them. `Strtod` (`value/libc.rs`) is the
byte-fed `strtod` a streamed `float8in` can take, `float.rs` and
`datetime/grammar.rs` reading by slices of their own. A `numeric` radix
spelling (v16 on) cannot be bounded by its decimal length while read. Each
datetime reader inlines `within_workbuf`'s refusal path, so a reader added
later inherits its per-call cost. And `KD176`.

**Why P40 cares.** Each is a reader or holding a phase streaming every reader
has to reach, or price, beyond the scalars above.

**Origin.** The line ledger's wrap, 2026-11-19, read off the code at `6f5f8228`.
